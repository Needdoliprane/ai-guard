//! Inert stdio peers for the held bootstrap and the egress helper.

use super::{IdV1, egress_wire};
use anyhow::{Result, ensure};
use tokio::{
    io::{AsyncRead, AsyncReadExt, AsyncWrite},
    sync::oneshot,
    time::{Duration, Instant, timeout},
};

const RELEASE_LIMIT: usize = 512;
const WITNESS_TIMEOUT: Duration = Duration::from_secs(10);

/// Notify on the first actual byte, then preserve the complete bounded input
/// verbatim through EOF, including bytes that would invalidate a release.
pub(super) async fn read_release(
    input: &mut (impl AsyncRead + Unpin),
    first_byte: oneshot::Sender<()>,
) -> Result<Vec<u8>> {
    timeout(WITNESS_TIMEOUT, async {
        let mut first = [0_u8; 1];
        if input.read(&mut first).await? == 0 {
            return Ok(Vec::new());
        }
        let _ = first_byte.send(());
        let mut bytes = first.to_vec();
        input
            .take(RELEASE_LIMIT as u64)
            .read_to_end(&mut bytes)
            .await?;
        ensure!(bytes.len() <= RELEASE_LIMIT, "held bootstrap stdin exceeds witness bound");
        Ok(bytes)
    })
    .await?
}

/// Echo only the helper handshake for the expected attempt. The caller keeps
/// the duplex alive. It carries neither bootstrap input nor upstream traffic.
pub(super) async fn echo_ready(
    stream: &mut (impl AsyncRead + AsyncWrite + Unpin),
    attempt: &IdV1,
) -> Result<egress_wire::CommonEgressHandshakeV1> {
    let deadline = Instant::now() + WITNESS_TIMEOUT;
    let hello = egress_wire::read_handshake(stream, deadline).await?;
    ensure!(&hello.binding_id == attempt, "helper HELLO binds another attempt");
    egress_wire::write_handshake(stream, &hello, deadline).await?;
    Ok(hello)
}
