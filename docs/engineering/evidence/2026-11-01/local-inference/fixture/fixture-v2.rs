use std::{
    io::{Read, Write},
    net::{TcpListener, TcpStream},
    thread,
    time::{Duration, Instant},
};

const ACCEPT_BUDGET: Duration = Duration::from_secs(120);
const IO_TIMEOUT: Duration = Duration::from_secs(2);

fn request_headers(stream: &mut TcpStream) -> std::io::Result<()> {
    let deadline = Instant::now() + Duration::from_secs(5);
    let mut all = Vec::new();
    let mut buf = [0u8; 1024];
    while !all.windows(4).any(|window| window == b"\r\n\r\n") {
        let remaining = deadline.saturating_duration_since(Instant::now());
        if remaining.is_zero() {
            return Err(std::io::Error::new(std::io::ErrorKind::TimedOut, "request header deadline"));
        }
        stream.set_read_timeout(Some(remaining.min(IO_TIMEOUT)))?;
        let read = stream.read(&mut buf)?;
        if read == 0 {
            return Err(std::io::Error::new(std::io::ErrorKind::UnexpectedEof, "request ended before headers"));
        }
        all.extend_from_slice(&buf[..read]);
        if all.len() > 16 * 1024 {
            return Err(std::io::Error::new(std::io::ErrorKind::InvalidData, "request headers exceed bound"));
        }
    }
    Ok(())
}

fn accept_bounded(listener: &TcpListener, deadline: Instant) -> std::io::Result<TcpStream> {
    loop {
        match listener.accept() {
            Ok((stream, _)) => return Ok(stream),
            Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                if Instant::now() >= deadline {
                    return Err(std::io::Error::new(std::io::ErrorKind::TimedOut, "global accept deadline"));
                }
                thread::sleep(Duration::from_millis(20));
            }
            Err(error) => return Err(error),
        }
    }
}

fn chunk(stream: &mut TcpStream, fragment: &[u8]) -> std::io::Result<()> {
    write!(stream, "{:x}\r\n", fragment.len())?;
    stream.write_all(fragment)?;
    stream.write_all(b"\r\n")
}

fn main() -> std::io::Result<()> {
    let listener = TcpListener::bind("0.0.0.0:8080")?;
    listener.set_nonblocking(true)?;
    let deadline = Instant::now() + ACCEPT_BUDGET;
    for case in 0..4 {
        let mut stream = accept_bounded(&listener, deadline)?;
        stream.set_nodelay(true)?;
        stream.set_write_timeout(Some(IO_TIMEOUT))?;
        if let Err(error) = request_headers(&mut stream) {
            eprintln!("case={case} setup=request-read error={error}");
            return Err(error);
        }
        match case {
            // Legacy probe is expected to close after headers; tolerate that client-side close.
            0 | 1 => {
                let response = b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\nConnection: close\r\n\r\n";
                if let Err(error) = stream.write_all(response) {
                    if case == 0 && matches!(error.kind(), std::io::ErrorKind::BrokenPipe | std::io::ErrorKind::ConnectionReset) {
                        eprintln!("case=0 expected-legacy-client-close=headers {error}");
                        continue;
                    }
                    eprintln!("case={case} response=headers unexpected-write-error={error}");
                    return Err(error);
                }
                thread::sleep(Duration::from_millis(100));
                let fragments: [&[u8]; 2] = [b"{\"ok\":", b"true}"];
                let mut write_error = None;
                for fragment in fragments {
                    if let Err(error) = chunk(&mut stream, fragment) {
                        write_error = Some(error);
                        break;
                    }
                    thread::sleep(Duration::from_millis(100));
                }
                if write_error.is_none() {
                    if let Err(error) = stream.write_all(b"0\r\n\r\n") {
                        write_error = Some(error);
                    }
                }
                if let Some(error) = write_error {
                    if case == 0 && matches!(error.kind(), std::io::ErrorKind::BrokenPipe | std::io::ErrorKind::ConnectionReset) {
                        eprintln!("case=0 expected-legacy-client-close={error}");
                    } else {
                        eprintln!("case=1 v3-positive-write-failed={error}");
                        return Err(error);
                    }
                }
            }
            2 => {
                stream.write_all(b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\nConnection: close\r\n\r\n5\r\nabc")?;
                // Drop at scope end: EOF is the intended truncated-body condition.
            }
            _ => {
                stream.write_all(b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\nConnection: close\r\n\r\n")?;
                thread::sleep(Duration::from_secs(7));
            }
        }
    }
    Ok(())
}
