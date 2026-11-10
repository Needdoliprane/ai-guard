//! A Ready inner future may finish in one poll after its absolute deadline.
//! These IO adapters cross a monotone condition inside that poll deliberately;
//! no assertion depends on a timer winning a scheduler race or on elapsed speed.
use super::*;
use std::{
    pin::Pin,
    sync::atomic::{AtomicBool, Ordering},
    task::{Context, Poll},
};
use tokio::io::ReadBuf;

fn cross_deadline(until: Instant, crossed: &AtomicBool) {
    while Instant::now() < until {
        std::thread::park_timeout(until.saturating_duration_since(Instant::now()));
    }
    crossed.store(true, Ordering::Release);
}
struct LateRead<R> {
    inner: R,
    until: Instant,
    crossed: std::sync::Arc<AtomicBool>,
}
impl<R: AsyncRead + Unpin> AsyncRead for LateRead<R> {
    fn poll_read(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buffer: &mut ReadBuf<'_>,
    ) -> Poll<std::io::Result<()>> {
        let this = self.get_mut();
        let before = buffer.filled().len();
        let result = Pin::new(&mut this.inner).poll_read(cx, buffer);
        if matches!(&result, Poll::Ready(Ok(())))
            && buffer.filled().len() > before
            && !this.crossed.load(Ordering::Acquire)
        {
            cross_deadline(this.until, &this.crossed);
        }
        result
    }
}
struct LateFlush<W> {
    inner: W,
    until: Instant,
    crossed: std::sync::Arc<AtomicBool>,
}
impl<W: AsyncWrite + Unpin> AsyncWrite for LateFlush<W> {
    fn poll_write(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        bytes: &[u8],
    ) -> Poll<std::io::Result<usize>> {
        Pin::new(&mut self.get_mut().inner).poll_write(cx, bytes)
    }
    fn poll_flush(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<std::io::Result<()>> {
        let this = self.get_mut();
        let result = Pin::new(&mut this.inner).poll_flush(cx);
        if matches!(&result, Poll::Ready(Ok(()))) {
            cross_deadline(this.until, &this.crossed);
        }
        result
    }
    fn poll_shutdown(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<std::io::Result<()>> {
        Pin::new(&mut self.get_mut().inner).poll_shutdown(cx)
    }
}

#[tokio::test]
async fn worker_refuses_a_ready_flush_that_completes_after_the_absolute_deadline() {
    let binding = WireBinding::from(&binding());
    let pool = std::sync::Arc::new(Semaphore::new(RESERVED_BYTES as usize));
    let crossed = std::sync::Arc::new(AtomicBool::new(false));
    let (worker, mut peer) = tokio::io::duplex(4096);
    write_frame(
        &mut peer,
        &Frame::Hello {
            version: 2,
            exchange_id: Uuid::new_v4(),
            binding,
        },
    )
    .await
    .unwrap();
    let (input, output) = tokio::io::split(worker);
    let capacity = capacity(&pool).await;
    let until = Instant::now() + Duration::from_millis(20);
    let result = bootstrap_waiting(
        input,
        LateFlush {
            inner: output,
            until,
            crossed: crossed.clone(),
        },
        capacity,
        until,
    )
    .await;
    assert!(
        crossed.load(Ordering::Acquire),
        "must exercise late Ready completion"
    );
    assert!(matches!(result, Err(BootstrapError::Deadline)));
    assert_eq!(pool.available_permits(), RESERVED_BYTES as usize);
}

#[tokio::test]
async fn controller_refuses_a_buffered_ready_polled_after_the_absolute_deadline() {
    let binding = binding();
    let pool = std::sync::Arc::new(Semaphore::new(RESERVED_BYTES as usize));
    let crossed = std::sync::Arc::new(AtomicBool::new(false));
    let (control, mut peer) = tokio::io::duplex(4096);
    let (input, output) = tokio::io::split(control);
    let capacity = capacity(&pool).await;
    let until = Instant::now() + Duration::from_millis(20);
    let owner = BootstrapIoOwner::start(
        binding,
        LateRead {
            inner: input,
            until,
            crossed: crossed.clone(),
        },
        output,
        capacity,
        until,
    )
    .unwrap();
    let Frame::Hello {
        version,
        exchange_id,
        binding,
    } = read_frame(&mut peer).await.unwrap()
    else {
        panic!("real controller must write Hello before the peer replies");
    };
    write_frame(
        &mut peer,
        &Frame::Ready {
            version,
            exchange_id,
            binding,
        },
    )
    .await
    .unwrap();
    let result = owner.join().await;
    assert!(
        crossed.load(Ordering::Acquire),
        "must exercise late buffered Ready poll"
    );
    assert!(matches!(result, Err(BootstrapError::Deadline)));
    assert_eq!(pool.available_permits(), RESERVED_BYTES as usize);
}
