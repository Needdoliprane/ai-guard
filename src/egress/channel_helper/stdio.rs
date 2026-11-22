//! Nonblocking owned standard-pipe adapters; no detached Tokio stdin task.
use super::{CommonEgressHelperError as Error, LoopbackConfig, Result};
use std::{
    fs::File,
    io,
    os::fd::{AsRawFd, FromRawFd},
    pin::Pin,
    task::{Context, Poll, ready},
};
use tokio::io::{AsyncRead, AsyncWrite, ReadBuf, unix::AsyncFd};

struct NonblockingFile(AsyncFd<File>);
impl NonblockingFile {
    fn new(file: File) -> Result<Self> {
        let mut facts = std::mem::MaybeUninit::<libc::stat>::uninit();
        // SAFETY: the owned descriptor is live and the output stat is writable.
        if unsafe { libc::fstat(file.as_raw_fd(), facts.as_mut_ptr()) } != 0 {
            return Err(Error::Unavailable);
        }
        // SAFETY: successful fstat initialized every stat field.
        let kind = unsafe { facts.assume_init() }.st_mode & libc::S_IFMT;
        if kind != libc::S_IFIFO && kind != libc::S_IFSOCK {
            return Err(Error::Protocol);
        }
        // SAFETY: fcntl only queries/sets flags on this retained descriptor.
        let flags = unsafe { libc::fcntl(file.as_raw_fd(), libc::F_GETFL) };
        if flags < 0
            || unsafe { libc::fcntl(file.as_raw_fd(), libc::F_SETFL, flags | libc::O_NONBLOCK) }
                != 0
        {
            return Err(Error::Unavailable);
        }
        // SAFETY: owned File retains this same nonblocking FD until deregistered.
        // This wrapper never closes, replaces or extracts its registered File.
        unsafe { AsyncFd::register(file) }
            .map(Self)
            .map_err(|_| Error::Unavailable)
    }
}
impl AsyncRead for NonblockingFile {
    fn poll_read(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buffer: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        loop {
            let mut guard = ready!(self.0.poll_read_ready(cx))?;
            match guard
                .try_io(|inner| io::Read::read(&mut inner.get_ref(), buffer.initialize_unfilled()))
            {
                Ok(Ok(count)) => {
                    buffer.advance(count);
                    return Poll::Ready(Ok(()));
                }
                Ok(Err(error)) => return Poll::Ready(Err(error)),
                Err(_) => continue,
            }
        }
    }
}
impl AsyncWrite for NonblockingFile {
    fn poll_write(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        bytes: &[u8],
    ) -> Poll<io::Result<usize>> {
        loop {
            let mut guard = ready!(self.0.poll_write_ready(cx))?;
            match guard.try_io(|inner| io::Write::write(&mut inner.get_ref(), bytes)) {
                Ok(result) => return Poll::Ready(result),
                Err(_) => continue,
            }
        }
    }
    fn poll_flush(self: Pin<&mut Self>, _: &mut Context<'_>) -> Poll<io::Result<()>> {
        Poll::Ready(Ok(()))
    }
    fn poll_shutdown(self: Pin<&mut Self>, _: &mut Context<'_>) -> Poll<io::Result<()>> {
        Poll::Ready(Ok(()))
    }
}

pub async fn serve_stdio(config: LoopbackConfig) -> Result<()> {
    let duplicate = |original| {
        // SAFETY: a successful duplicate is independently owned; stdin/stdout
        // themselves are neither adopted nor closed by this wrapper.
        let fd = unsafe { libc::fcntl(original, libc::F_DUPFD_CLOEXEC, 3) };
        if fd < 0 {
            return Err(Error::Unavailable);
        }
        // SAFETY: F_DUPFD_CLOEXEC transferred the new descriptor's ownership.
        NonblockingFile::new(unsafe { File::from_raw_fd(fd) })
    };
    super::serve(config, duplicate(0)?, duplicate(1)?).await
}

/// Fixed code only, best effort/nonblocking; never includes remote IO text.
pub fn emit_diagnostic(error: Error) {
    let bytes = error.diagnostic().as_bytes();
    // SAFETY: fixed stderr descriptor and a static byte slice; a full pipe
    // cannot hold shutdown hostage and no input is used as a format string.
    unsafe {
        let flags = libc::fcntl(2, libc::F_GETFL);
        if flags >= 0 && libc::fcntl(2, libc::F_SETFL, flags | libc::O_NONBLOCK) == 0 {
            let _ = libc::write(2, bytes.as_ptr().cast(), bytes.len());
        }
    }
}
