use super::*;
use crate::relay::channel::{
    ChannelLimitsV1, CloseReasonV1, CloseV1, ErrorCodeV1, InboxRequestV1, OperationV1,
    RequestPayloadV1, ResponseResultV1, ResponseV1,
};
use std::os::unix::fs::PermissionsExt;
use tokio::{net::UnixStream, task::JoinHandle};

const CHANNEL: &str = "10000000-0000-4000-8000-000000000001";
const CORRELATION: &str = "20000000-0000-4000-8000-000000000001";

fn hello() -> HandshakeV1 {
    HandshakeV1 {
        version: 1,
        channel_id: CHANNEL.into(),
        epoch: 9,
        limits: ChannelLimitsV1::default(),
        helper_version: 1,
    }
}

fn request(correlation: &str) -> RequestV1 {
    RequestV1 {
        version: 1,
        channel_id: CHANNEL.into(),
        epoch: 9,
        correlation_id: correlation.into(),
        payload: RequestPayloadV1::Inbox(InboxRequestV1 {
            after: None,
            limit: 1,
        }),
    }
}

fn response(correlation: &str) -> FrameV1 {
    FrameV1::Response(ResponseV1 {
        version: 1,
        channel_id: CHANNEL.into(),
        epoch: 9,
        correlation_id: correlation.into(),
        operation: OperationV1::Inbox,
        result: ResponseResultV1::Error(ErrorCodeV1::Forbidden),
    })
}

fn fixture_root() -> (tempfile::TempDir, Directory) {
    let temporary = tempfile::Builder::new()
        .prefix("relay-h-")
        .tempdir()
        .unwrap();
    std::fs::set_permissions(temporary.path(), std::fs::Permissions::from_mode(0o700)).unwrap();
    let directory = Directory::open_existing(&temporary.path().canonicalize().unwrap()).unwrap();
    (temporary, directory)
}

fn start(root: Directory) -> (UnixStream, JoinHandle<Result<(), HelperError>>) {
    let (control, helper) = UnixStream::pair().unwrap();
    let (input, output) = helper.into_split();
    let task = tokio::spawn(serve(root, input, output));
    (control, task)
}

async fn join(mut task: JoinHandle<Result<(), HelperError>>) -> Result<(), HelperError> {
    match tokio::time::timeout(Duration::from_secs(3), &mut task).await {
        Ok(result) => result.unwrap(),
        Err(_) => {
            task.abort();
            let _ = task.await;
            panic!("helper owner did not close");
        }
    }
}

async fn ready(control: &mut UnixStream, root: &Directory) -> DiscoveredHelper {
    write_async(control, &FrameV1::Hello(hello()))
        .await
        .unwrap();
    let mut reader = AsyncFrameReader::new();
    assert!(matches!(
        reader.read(control, FrameLimit::Control).await.unwrap(),
        Some(FrameV1::Ready(_))
    ));
    let deadline = Instant::now() + Duration::from_secs(2);
    loop {
        for entry in std::fs::read_dir(root.path()).unwrap() {
            let entry = entry.unwrap();
            let name = entry.file_name().into_string().unwrap();
            if let Ok(record) = discover(root, &name) {
                return record;
            }
        }
        assert!(Instant::now() < deadline);
        tokio::task::yield_now().await;
    }
}

#[tokio::test]
async fn no_namespace_is_published_before_handshake_and_eof_joins_owner() {
    let (_temporary, root) = fixture_root();
    let (control, task) = start(root.clone());
    tokio::task::yield_now().await;
    assert_eq!(std::fs::read_dir(root.path()).unwrap().count(), 0);
    drop(control);
    assert!(join(task).await.is_ok());
    assert_eq!(std::fs::read_dir(root.path()).unwrap().count(), 0);
}

#[tokio::test]
async fn local_uds_roundtrip_is_closed_and_control_eof_closes_idle_client() {
    let (_temporary, root) = fixture_root();
    let (mut control, task) = start(root.clone());
    let record = ready(&mut control, &root).await;
    assert_eq!(record.channel_id(), CHANNEL);
    assert_eq!(record.epoch(), 9);
    let namespace = root.path().join(record.directory_name());
    assert_eq!(
        std::fs::metadata(&namespace).unwrap().permissions().mode() & 0o777,
        0o700
    );
    assert_eq!(
        std::fs::metadata(namespace.join("s"))
            .unwrap()
            .permissions()
            .mode()
            & 0o777,
        0o600
    );
    assert_eq!(
        std::fs::metadata(namespace.join(RECORD_NAME))
            .unwrap()
            .permissions()
            .mode()
            & 0o777,
        0o600
    );
    let mut client = record.connect_checked(&root).await.unwrap();
    write_async(&mut client, &FrameV1::Request(request(CORRELATION)))
        .await
        .unwrap();
    let mut reader = AsyncFrameReader::new();
    assert!(matches!(
        reader
            .read(&mut control, FrameLimit::Request)
            .await
            .unwrap(),
        Some(FrameV1::Request(_))
    ));
    write_async(&mut control, &response(CORRELATION))
        .await
        .unwrap();
    assert!(matches!(
        reader
            .read(&mut client, FrameLimit::Response)
            .await
            .unwrap(),
        Some(FrameV1::Response(_))
    ));
    drop(control);
    assert!(join(task).await.is_ok());
    let mut byte = [0; 1];
    assert_eq!(client.read(&mut byte).await.unwrap(), 0);
    assert!(record.connect_checked(&root).await.is_err());
    assert!(namespace.join(RECORD_NAME).exists());
}

#[tokio::test]
async fn pending_request_closes_second_connection_before_body_and_preserves_first_exchange() {
    let (_temporary, root) = fixture_root();
    let (mut control, task) = start(root.clone());
    let record = ready(&mut control, &root).await;
    let mut first = record.connect_checked(&root).await.unwrap();
    write_async(&mut first, &FrameV1::Request(request(CORRELATION)))
        .await
        .unwrap();
    let mut reader = AsyncFrameReader::new();
    assert!(matches!(
        reader
            .read(&mut control, FrameLimit::Request)
            .await
            .unwrap(),
        Some(FrameV1::Request(_))
    ));
    let mut second = record.connect_checked(&root).await.unwrap();
    // Acceptance may close before this prefix is written; either ordering
    // still forbids reading a body or dispatching a second operation.
    let _ = second.write_all(&278_528_u32.to_be_bytes()).await;
    let mut byte = [0; 1];
    let closed = tokio::time::timeout(Duration::from_secs(2), second.read(&mut byte))
        .await
        .unwrap();
    assert!(matches!(closed, Ok(0) | Err(_)));
    // The first request is still outstanding; no fabricated busy response and
    // no second request is submitted. A real control response resolves it.
    write_async(&mut control, &response(CORRELATION))
        .await
        .unwrap();
    assert!(matches!(
        reader.read(&mut first, FrameLimit::Response).await.unwrap(),
        Some(FrameV1::Response(_))
    ));
    let close = FrameV1::Close(CloseV1 {
        version: 1,
        channel_id: CHANNEL.into(),
        epoch: 9,
        reason: CloseReasonV1::Shutdown,
    });
    write_async(&mut control, &close).await.unwrap();
    assert!(join(task).await.is_ok());
}

#[tokio::test]
async fn pending_control_eof_joins_with_bounded_ready_extra_connections() {
    use tokio::sync::{oneshot, watch};

    const EXTRA_CONNECTIONS: usize = 16;
    const EXTRA_ENCODED_BYTES: usize = 16 * 1024;
    let (_temporary, root) = fixture_root();
    let (mut control, helper) = UnixStream::pair().unwrap();
    let (input, output) = helper.into_split();
    let helper_root = root.clone();
    let (pause, mut paused) = watch::channel(false);
    let (parked, parking_confirmed) = oneshot::channel();
    // This test-only owner gate stops polling the real serve future after the
    // first Request has actually crossed the control socket. It lets EOF and a
    // small, finite accept queue both be ready before serve is polled again.
    // No production branch, IO result, timer or transport is replaced.
    let mut task = tokio::spawn(async move {
        let serving = serve(helper_root, input, output);
        tokio::pin!(serving);
        let mut parked = Some(parked);
        loop {
            let is_paused = *paused.borrow_and_update();
            if is_paused {
                if let Some(parked) = parked.take() {
                    let _ = parked.send(());
                }
                paused
                    .changed()
                    .await
                    .map_err(|_| HelperError::Unavailable)?;
                continue;
            }
            tokio::select! {
                biased;
                changed = paused.changed() => {
                    changed.map_err(|_| HelperError::Unavailable)?;
                }
                result = &mut serving => return result,
            }
        }
    });
    let record = ready(&mut control, &root).await;
    let mut original = record.connect_checked(&root).await.unwrap();
    write_async(&mut original, &FrameV1::Request(request(CORRELATION)))
        .await
        .unwrap();
    let mut reader = AsyncFrameReader::new();
    assert!(matches!(
        reader
            .read(&mut control, FrameLimit::Request)
            .await
            .unwrap(),
        Some(FrameV1::Request(_))
    ));
    pause.send(true).unwrap();
    tokio::time::timeout(Duration::from_secs(2), parking_confirmed)
        .await
        .unwrap()
        .unwrap();

    // The parent itself owns this bounded producer; there are no connection
    // tasks to leave running. All connections target this new fixture UDS.
    let mut extras = Vec::with_capacity(EXTRA_CONNECTIONS);
    let mut encoded = 0;
    for index in 0..EXTRA_CONNECTIONS {
        let mut connection =
            tokio::time::timeout(Duration::from_secs(2), record.connect_checked(&root))
                .await
                .unwrap()
                .unwrap();
        let correlation = format!("20000000-0000-4000-8000-{:012x}", index + 2);
        let frame = FrameV1::Request(request(&correlation));
        encoded += 4 + frame.encode().unwrap().len();
        assert!(encoded <= EXTRA_ENCODED_BYTES);
        write_async(&mut connection, &frame).await.unwrap();
        extras.push(connection);
    }
    // SHUT_WR supplies real control EOF while preserving the other direction
    // so the test can observe whether a second Request was ever dispatched.
    control.shutdown().await.unwrap();
    tokio::task::yield_now().await;
    pause.send(false).unwrap();
    let joined = tokio::time::timeout(Duration::from_secs(5), &mut task).await;
    let completed = match joined {
        Ok(result) => Some(result.unwrap()),
        Err(_) => {
            task.abort();
            let _ = task.await;
            None
        }
    };
    // Clean up and join the owner before assessing the discriminating oracle.
    let mut byte = [0; 1];
    let extra_dispatch =
        tokio::time::timeout(Duration::from_secs(1), control.read(&mut byte)).await;
    let clients_closed = tokio::time::timeout(Duration::from_secs(2), async {
        if original
            .read(&mut byte)
            .await
            .map_or(true, |count| count != 0)
        {
            return false;
        }
        for connection in &mut extras {
            if matches!(connection.read(&mut byte).await, Ok(count) if count != 0) {
                return false;
            }
        }
        true
    })
    .await;
    drop(extras);
    drop(original);
    drop(control);
    assert!(
        matches!(completed, Some(Ok(()))),
        "pending helper did not join within five seconds after ready control EOF"
    );
    assert!(
        matches!(extra_dispatch, Ok(Ok(0))),
        "a second dispatch or retained control writer followed ready EOF"
    );
    assert!(
        matches!(clients_closed, Ok(true)),
        "helper retained a client after owner completion"
    );
}

#[tokio::test]
async fn partial_control_frame_survives_other_ready_branches() {
    let (_temporary, root) = fixture_root();
    let (mut control, task) = start(root.clone());
    let record = ready(&mut control, &root).await;
    let mut first = record.connect_checked(&root).await.unwrap();
    write_async(&mut first, &FrameV1::Request(request(CORRELATION)))
        .await
        .unwrap();
    let mut reader = AsyncFrameReader::new();
    reader
        .read(&mut control, FrameLimit::Request)
        .await
        .unwrap();
    let body = response(CORRELATION).encode().unwrap();
    let prefix = (body.len() as u32).to_be_bytes();
    control.write_all(&prefix[..2]).await.unwrap();
    tokio::task::yield_now().await;
    let mut second = record.connect_checked(&root).await.unwrap();
    let mut byte = [0; 1];
    assert!(matches!(
        tokio::time::timeout(Duration::from_secs(2), second.read(&mut byte))
            .await
            .unwrap(),
        Ok(0) | Err(_)
    ));
    control.write_all(&prefix[2..]).await.unwrap();
    control.write_all(&body).await.unwrap();
    assert!(matches!(
        reader.read(&mut first, FrameLimit::Response).await.unwrap(),
        Some(FrameV1::Response(_))
    ));
    drop(control);
    assert!(join(task).await.is_ok());
}

#[tokio::test]
async fn second_frame_on_pending_client_is_closed_without_dispatching_its_body() {
    let (_temporary, root) = fixture_root();
    let (mut control, task) = start(root.clone());
    let record = ready(&mut control, &root).await;
    let mut client = record.connect_checked(&root).await.unwrap();
    write_async(&mut client, &FrameV1::Request(request(CORRELATION)))
        .await
        .unwrap();
    let mut reader = AsyncFrameReader::new();
    reader
        .read(&mut control, FrameLimit::Request)
        .await
        .unwrap();
    client.write_all(&278_528_u32.to_be_bytes()).await.unwrap();
    let mut byte = [0; 1];
    let closed = tokio::time::timeout(Duration::from_secs(2), client.read(&mut byte))
        .await
        .unwrap();
    assert!(matches!(closed, Ok(0) | Err(_)));
    assert!(
        matches!(control.try_read(&mut byte), Err(error) if error.kind() == io::ErrorKind::WouldBlock)
    );
    // Closing the application does not cancel or fabricate the first result.
    write_async(&mut control, &response(CORRELATION))
        .await
        .unwrap();
    let close = FrameV1::Close(CloseV1 {
        version: 1,
        channel_id: CHANNEL.into(),
        epoch: 9,
        reason: CloseReasonV1::Shutdown,
    });
    write_async(&mut control, &close).await.unwrap();
    assert!(join(task).await.is_ok());
}

#[tokio::test]
async fn incomplete_control_frame_expires_and_joins_while_application_is_idle() {
    let (_temporary, root) = fixture_root();
    let (mut control, task) = start(root.clone());
    let record = ready(&mut control, &root).await;
    let mut client = record.connect_checked(&root).await.unwrap();
    control.write_all(&[0]).await.unwrap();
    assert!(matches!(join(task).await, Err(HelperError::Timeout)));
    let mut byte = [0; 1];
    assert_eq!(client.read(&mut byte).await.unwrap(), 0);
    assert!(record.connect_checked(&root).await.is_err());
}

#[tokio::test]
async fn wrong_epoch_response_terminates_without_delivering_or_reopening() {
    let (_temporary, root) = fixture_root();
    let (mut control, task) = start(root.clone());
    let record = ready(&mut control, &root).await;
    let mut client = record.connect_checked(&root).await.unwrap();
    write_async(&mut client, &FrameV1::Request(request(CORRELATION)))
        .await
        .unwrap();
    let mut reader = AsyncFrameReader::new();
    reader
        .read(&mut control, FrameLimit::Request)
        .await
        .unwrap();
    let FrameV1::Response(mut wrong) = response(CORRELATION) else {
        unreachable!()
    };
    wrong.epoch += 1;
    write_async(&mut control, &FrameV1::Response(wrong))
        .await
        .unwrap();
    assert!(matches!(join(task).await, Err(HelperError::Protocol)));
    let mut byte = [0; 1];
    assert_eq!(client.read(&mut byte).await.unwrap(), 0);
    assert!(record.connect_checked(&root).await.is_err());
}

#[tokio::test]
async fn substituted_discovery_and_socket_entries_are_refused_without_repairs() {
    use std::os::unix::fs::symlink;
    let (_temporary, root) = fixture_root();
    let (mut control, task) = start(root.clone());
    let record = ready(&mut control, &root).await;
    let namespace = root.path().join(record.directory_name());
    std::fs::rename(namespace.join(RECORD_NAME), namespace.join("record-held")).unwrap();
    symlink("record-held", namespace.join(RECORD_NAME)).unwrap();
    assert!(record.connect_checked(&root).await.is_err());
    assert!(discover(&root, record.directory_name()).is_err());
    assert!(
        namespace
            .join(RECORD_NAME)
            .symlink_metadata()
            .unwrap()
            .file_type()
            .is_symlink()
    );
    drop(control);
    assert!(join(task).await.is_ok());
    assert!(namespace.join("record-held").exists());
}

#[tokio::test]
async fn generation_cap_and_exclusive_helper_do_not_adopt_or_remove_existing_entries() {
    let (_temporary, root) = fixture_root();
    for _ in 0..128 {
        drop(PrivateListener::bind_new(&root).unwrap());
    }
    assert!(PrivateListener::bind_new(&root).is_err());
    assert_eq!(std::fs::read_dir(root.path()).unwrap().count(), 128);
    let (mut control, task) = start(root.clone());
    write_async(&mut control, &FrameV1::Hello(hello()))
        .await
        .unwrap();
    assert!(join(task).await.is_err());
    assert_eq!(std::fs::read_dir(root.path()).unwrap().count(), 128);

    let (_temporary, root) = fixture_root();
    let (mut control, first) = start(root.clone());
    let _record = ready(&mut control, &root).await;
    let (second_control, second) = start(root.clone());
    assert!(matches!(join(second).await, Err(HelperError::Unavailable)));
    drop(second_control);
    drop(control);
    assert!(join(first).await.is_ok());
    assert_eq!(std::fs::read_dir(root.path()).unwrap().count(), 1);
}

#[tokio::test]
async fn async_fd_pipe_eof_wakes_reader_without_a_blocking_stdin_thread() {
    let mut fds = [0; 2];
    // SAFETY: the array has room for the two newly owned pipe descriptors.
    assert_eq!(unsafe { libc::pipe(fds.as_mut_ptr()) }, 0);
    // SAFETY: successful pipe transfers distinct owned descriptors exactly once.
    let input = unsafe { File::from_raw_fd(fds[0]) };
    // SAFETY: the write end is independently owned by this test.
    let mut output = unsafe { File::from_raw_fd(fds[1]) };
    let mut input = NonblockingFile::new(input).unwrap();
    std::io::Write::write_all(&mut output, b"one").unwrap();
    let mut bytes = [0; 3];
    input.read_exact(&mut bytes).await.unwrap();
    assert_eq!(&bytes, b"one");
    drop(output);
    assert_eq!(input.read(&mut bytes).await.unwrap(), 0);
}
