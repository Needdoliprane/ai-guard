//! The software receiver and helper keep separate ownership of their pipes.
#![cfg(unix)]

use agentsandbox::egress::{
    channel_protocol as egress_wire,
    ids::{CHANNEL_CONTRACT, DigestV1, IdV1},
};
use anyhow::Result;
use tokio::{
    io::{AsyncWriteExt, duplex},
    sync::oneshot,
    time::{Duration, Instant, timeout},
};

#[path = "support/held_egress_witness.rs"]
mod witness;

#[tokio::test]
async fn held_egress_ready_does_not_consume_bootstrap_stdin() -> Result<()> {
    let attempt = IdV1::random();
    let frame = format!("AI-GUARD-EGRESS-RELEASE-V1 {}\n", attempt.as_str());
    let (mut release, mut receiver) = duplex(128);
    let (mut control, mut helper) = duplex(4096);
    let (started, mut first_byte) = oneshot::channel();
    let capture = tokio::spawn(async move { witness::read_release(&mut receiver, started).await });
    let hello = egress_wire::CommonEgressHandshakeV1 {
        version: 1,
        protocol: CHANNEL_CONTRACT.into(),
        claim_id: IdV1::random(),
        binding_id: attempt.clone(),
        transport_epoch: 1,
        context_digest: DigestV1::of(b"held egress witness"),
        challenge: IdV1::random(),
        helper_version: 1,
        capability: Default::default(),
    };
    let deadline = Instant::now() + Duration::from_secs(3);
    let ready = tokio::spawn(async move { witness::echo_ready(&mut helper, &attempt).await });
    egress_wire::write_handshake(&mut control, &hello, deadline).await?;
    assert_eq!(
        egress_wire::read_handshake(&mut control, deadline).await?,
        hello
    );
    assert_eq!(ready.await??, hello);
    assert_eq!(first_byte.try_recv(), Err(oneshot::error::TryRecvError::Empty));

    release.write_all(frame.as_bytes()).await?;
    timeout(Duration::from_secs(3), &mut first_byte).await??;
    assert!(
        !capture.is_finished(),
        "one byte must not count as a complete frame"
    );
    release.shutdown().await?;
    assert_eq!(
        timeout(Duration::from_secs(3), capture).await???,
        frame.as_bytes()
    );
    Ok(())
}
