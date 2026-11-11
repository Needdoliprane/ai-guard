//! Real software HTTP/control/helper/UDS seam, with declared HTTP replies.
//! Real gateway/SQLite retention is exercised separately by relay_fairness.
use super::*;
use crate::relay::channel::{InboxRequestV1, helper};
use crate::relay::{RelayRefusalScopeV1, RelayRetryActionV1};

async fn exchange(
    client: &mut UnixStream,
    hello: &HandshakeV1,
    version: u32,
    payload: RequestPayloadV1,
) -> ResponseV1 {
    let request = RequestV1 {
        version,
        channel_id: hello.channel_id.clone(),
        epoch: hello.epoch,
        correlation_id: uuid::Uuid::new_v4().to_string(),
        payload,
    };
    checked_write(
        client,
        &FrameV1::Request(request.clone()),
        &Secrets(vec![PROOF.into()]),
    )
    .await
    .unwrap();
    let Some(FrameV1::Response(response)) = read_frame(client, FrameLimit::Response).await.unwrap()
    else {
        panic!("helper did not deliver response");
    };
    assert_eq!(response.version, version);
    assert_eq!(response.channel_id, request.channel_id);
    assert_eq!(response.epoch, request.epoch);
    assert_eq!(response.correlation_id, request.correlation_id);
    assert_eq!(response.operation, request.operation());
    response
}

#[tokio::test]
async fn typed_send_reaches_the_actual_helper_without_erasing_a_prior_unknown() {
    let fixture = Arc::new(HttpFixture::default());
    fixture.lose_first.store(true, Ordering::SeqCst);
    let server = Server::start(fixture.clone()).await;
    let (_temporary, root) = private_root();
    *fixture.before_effect.lock().unwrap() = Some(root.path().into());
    let context = FixtureContext::new(&server.origin, PROOF, endpoint());
    let role = Arc::new(RoleLease::acquire(&root).unwrap());
    let outbox = Outbox::create(&root, &context.scope, &context.secrets).unwrap();
    let io = context.io(outbox, role);
    let mut owner = context.owner(io.clone(), 1);
    let hello = owner.hello.clone();
    let (_helper_temporary, helper_root) = private_root();
    let (control, helper_stream) = UnixStream::pair().unwrap();
    let (control_input, control_output) = control.into_split();
    let (helper_input, helper_output) = helper_stream.into_split();
    let (stop, stopped) = oneshot::channel();
    let mut owner_task = tokio::spawn(async move {
        let result = owner.serve(control_input, control_output, stopped).await;
        (owner, result)
    });
    let served_root = helper_root.clone();
    let mut helper_task =
        tokio::spawn(async move { helper::serve(served_root, helper_input, helper_output).await });
    let discovery_deadline = Instant::now() + Duration::from_secs(5);
    let discovered = loop {
        let found = std::fs::read_dir(helper_root.path())
            .unwrap()
            .filter_map(|entry| {
                let name = entry.unwrap().file_name().into_string().unwrap();
                helper::discover(&helper_root, &name).ok()
            })
            .next();
        if let Some(found) = found {
            break found;
        }
        assert!(
            Instant::now() < discovery_deadline,
            "helper discovery deadline"
        );
        tokio::task::yield_now().await;
    };
    assert_eq!(discovered.channel_id(), hello.channel_id);
    assert_eq!(discovered.epoch(), hello.epoch);
    let mut client = discovered.connect_checked(&helper_root).await.unwrap();
    let inbox_payload = || {
        RequestPayloadV1::Inbox(InboxRequestV1 {
            after: None,
            limit: 1,
        })
    };
    assert!(matches!(
        exchange(&mut client, &hello, 1, inbox_payload())
            .await
            .result,
        ResponseResultV1::Ok(ReplyV1::Inbox(_))
    ));
    // ACK keeps its own ID and receipt validation before any unknown send.
    let ack = RelayAckV1 {
        version: 1,
        operation_id: uuid::Uuid::new_v4().to_string(),
        message_id: uuid::Uuid::new_v4().to_string(),
    };
    let ack_response = exchange(&mut client, &hello, 1, RequestPayloadV1::Ack(ack.clone())).await;
    assert!(
        matches!(ack_response.result, ResponseResultV1::Ok(ReplyV1::Ack(value))
        if value.operation_id == ack.operation_id && value.resource_id == ack.message_id)
    );

    let payload = send();
    let RequestPayloadV1::Send(input) = &payload else {
        unreachable!()
    };
    // A malformed accepted HTTP response leaves the effect unresolved.
    assert!(matches!(
        exchange(&mut client, &hello, 2, payload.clone())
            .await
            .result,
        ResponseResultV1::Error(ErrorCodeV1::Protocol)
    ));
    assert_eq!(fixture.calls.load(Ordering::SeqCst), 1);
    let operation = root
        .path()
        .join("relay_v1")
        .join(format!("op-{}", input.operation_id));
    let prepared = std::fs::read(operation.join("request.json")).unwrap();
    let dispatched = std::fs::read(operation.join("dispatch.json")).unwrap();
    fixture.typed_capacity.store(true, Ordering::SeqCst);
    let typed = exchange(&mut client, &hello, 2, payload.clone()).await;
    let encoded = FrameV1::Response(typed.clone()).encode().unwrap();
    assert!(
        !String::from_utf8(encoded)
            .unwrap()
            .contains("remote-private-diagnostic")
    );
    let ResponseResultV1::SendRefusal(detail) = typed.result else {
        panic!("typed refusal lost");
    };
    assert_eq!(detail.scope, RelayRefusalScopeV1::Sender);
    assert_eq!(detail.retry_action, RelayRetryActionV1::ContactOperator);
    assert_eq!(detail.own.as_ref().unwrap().retained.messages, 1);
    assert_eq!(
        detail.own.as_ref().unwrap().requested_bytes,
        u64::try_from(input.body.len()).unwrap()
    );
    assert_eq!(
        fixture.calls.load(Ordering::SeqCst),
        2,
        "no automatic retry"
    );
    let legacy = exchange(&mut client, &hello, 1, payload.clone()).await;
    assert!(matches!(
        legacy.result,
        ResponseResultV1::Error(ErrorCodeV1::Capacity)
    ));
    let encoded = FrameV1::Response(legacy).encode().unwrap();
    assert!(
        serde_json::from_slice::<Value>(&encoded)
            .unwrap()
            .get("refusal")
            .is_none()
    );
    assert!(matches!(
        exchange(&mut client, &hello, 1, inbox_payload())
            .await
            .result,
        ResponseResultV1::Ok(ReplyV1::Inbox(_))
    ));
    assert_eq!(fixture.calls.load(Ordering::SeqCst), 3);
    assert_eq!(
        std::fs::read(operation.join("request.json")).unwrap(),
        prepared
    );
    assert_eq!(
        std::fs::read(operation.join("dispatch.json")).unwrap(),
        dispatched
    );
    assert!(!operation.join("receipt.json").exists());
    let inputs = fixture.inputs.lock().unwrap();
    assert_eq!(inputs.len(), 3);
    assert!(
        inputs
            .iter()
            .all(|value| value == &serde_json::to_value(input).unwrap())
    );
    drop(inputs);

    stop.send(CloseReasonV1::Shutdown).unwrap();
    timeout_at(Instant::now() + Duration::from_secs(5), &mut helper_task)
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    let (owner, result) = timeout_at(Instant::now() + Duration::from_secs(5), &mut owner_task)
        .await
        .unwrap()
        .unwrap();
    result.unwrap();
    owner.io.join_pending().await.unwrap();
    drop(owner);
    drop(io);
    drop(client);
    let reopened = Outbox::open(&root, &context.scope, &context.secrets).unwrap();
    assert!(reopened.unresolved(&context.binding).unwrap());
    server.finish().await;
}
