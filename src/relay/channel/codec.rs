//! Length-prefix codec only. The transport owner supplies absolute and idle
//! deadlines, owns/joins the transport, and closes its FDs when this codec fails.
use super::{
    ChannelError, FrameV1, MAX_CONTROL_FRAME_BYTES, MAX_REQUEST_FRAME_BYTES,
    MAX_RESPONSE_FRAME_BYTES,
};
use std::io::{ErrorKind, Read, Write};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FrameLimit {
    Control,
    Request,
    Response,
}
impl FrameLimit {
    fn maximum(self) -> usize {
        match self {
            Self::Control => MAX_CONTROL_FRAME_BYTES,
            Self::Request => MAX_REQUEST_FRAME_BYTES,
            Self::Response => MAX_RESPONSE_FRAME_BYTES,
        }
    }

    pub(super) fn accepts(self, frame: &FrameV1) -> bool {
        match self {
            Self::Control => matches!(
                frame,
                FrameV1::Hello(_) | FrameV1::Ready(_) | FrameV1::Close(_)
            ),
            Self::Request => matches!(frame, FrameV1::Request(_)),
            Self::Response => matches!(frame, FrameV1::Response(_) | FrameV1::Close(_)),
        }
    }
}

/// Any framing/decoding/IO error permanently poisons this reader. No scanning
/// for a later delimiter or frame attempts to resynchronize untrusted bytes.
pub struct FrameReader<R> {
    input: R,
    closed: bool,
}
impl<R: Read> FrameReader<R> {
    pub fn new(input: R) -> Self {
        Self {
            input,
            closed: false,
        }
    }

    pub fn read(&mut self, limit: FrameLimit) -> Result<Option<FrameV1>, ChannelError> {
        if self.closed {
            return Err(ChannelError::Closed);
        }
        let result = self.read_inner(limit);
        if !matches!(&result, Ok(Some(_))) {
            self.closed = true;
        }
        result
    }

    fn read_inner(&mut self, limit: FrameLimit) -> Result<Option<FrameV1>, ChannelError> {
        let mut prefix = [0_u8; 4];
        loop {
            match self.input.read(&mut prefix[..1]) {
                Ok(0) => return Ok(None),
                Ok(_) => break,
                Err(error) if error.kind() == ErrorKind::Interrupted => continue,
                Err(_) => return Err(ChannelError::Io),
            }
        }
        self.input.read_exact(&mut prefix[1..]).map_err(io_error)?;
        let length = u32::from_be_bytes(prefix) as usize;
        // Check the advertised size before any allocation or payload read.
        if length == 0 || length > limit.maximum() {
            return Err(ChannelError::FrameSize);
        }
        let mut bytes = Vec::new();
        bytes
            .try_reserve_exact(length)
            .map_err(|_| ChannelError::Capacity)?;
        bytes.resize(length, 0);
        self.input.read_exact(&mut bytes).map_err(io_error)?;
        FrameV1::decode(&bytes, limit).map(Some)
    }

    pub fn close(&mut self) {
        self.closed = true;
    }

    /// Recover the owned transport for explicit close/join by its owner.
    pub fn into_inner(self) -> R {
        self.input
    }
}

pub struct FrameWriter<W> {
    output: W,
    closed: bool,
}
impl<W: Write> FrameWriter<W> {
    pub fn new(output: W) -> Self {
        Self {
            output,
            closed: false,
        }
    }

    pub fn write(&mut self, frame: &FrameV1) -> Result<(), ChannelError> {
        if self.closed {
            return Err(ChannelError::Closed);
        }
        let result = self.write_inner(frame);
        if result.is_err() {
            self.closed = true;
        }
        result
    }

    fn write_inner(&mut self, frame: &FrameV1) -> Result<(), ChannelError> {
        let bytes = frame.encode()?;
        let length = u32::try_from(bytes.len()).map_err(|_| ChannelError::FrameSize)?;
        self.output
            .write_all(&length.to_be_bytes())
            .map_err(io_error)?;
        self.output.write_all(&bytes).map_err(io_error)?;
        self.output.flush().map_err(io_error)
    }

    pub fn into_inner(self) -> W {
        self.output
    }
}

fn io_error(error: std::io::Error) -> ChannelError {
    if error.kind() == ErrorKind::UnexpectedEof {
        ChannelError::PartialFrame
    } else {
        ChannelError::Io
    }
}
