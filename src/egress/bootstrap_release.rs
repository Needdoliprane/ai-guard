//! Single V1 message on the retained bootstrap's exclusive, non-TTY stdin.
//! The challenge is public attempt data. Matching it proves only what the
//! worker read. The broker owns authority and the one-shot write.

use super::ids::IdV1;
use std::io::{ErrorKind, Read};

/// Explicit held mode. Set to the attempt's canonical UUID with the existing
/// loopback proxy marker. Never copy it into the provider's environment.
pub(crate) const HELD_CHALLENGE_ENV: &str = "AI_GUARD_EGRESS_HELD_V1";

const PREFIX: &[u8] = b"AI-GUARD-EGRESS-RELEASE-V1 ";
const FRAME_BYTES: usize = PREFIX.len() + 36 + 1;
const _: () = assert!(FRAME_BYTES < 512);

/// Encode exactly one frame. The sender closes its write endpoint after the
/// single write. Both the complete frame and EOF are required at the receiver.
pub(crate) fn encode_frame(challenge: &IdV1) -> [u8; FRAME_BYTES] {
    let mut frame = [b'\n'; FRAME_BYTES];
    frame[..PREFIX.len()].copy_from_slice(PREFIX);
    frame[PREFIX.len()..FRAME_BYTES - 1].copy_from_slice(challenge.as_str().as_bytes());
    frame
}

pub(crate) fn receive_frame(mut input: impl Read, challenge: &IdV1) -> bool {
    let mut frame = [0; FRAME_BYTES];
    if input.read_exact(&mut frame).is_err() || frame != encode_frame(challenge) {
        return false;
    }
    let mut trailing = [0];
    loop {
        match input.read(&mut trailing) {
            Ok(0) => return true,
            Err(error) if error.kind() == ErrorKind::Interrupted => continue,
            _ => return false,
        }
    }
}
