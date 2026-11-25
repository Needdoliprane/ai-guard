//! Software-only runtime witness. It never invokes Docker, Podman, a shell or
//! the requested workload. The private test controller owns synthetic facts.
use std::{
    fs,
    io::{self, Read, Write},
    net::Shutdown,
    os::unix::net::UnixStream,
};

fn run() -> io::Result<i32> {
    let executable = std::env::current_exe()?;
    let socket = fs::read_to_string(executable.with_extension("fixture"))?;
    let mut channel = UnixStream::connect(socket.trim())?;
    let arguments: Vec<String> = std::env::args().skip(1).collect();
    if arguments.len() > 256 || arguments.iter().map(String::len).sum::<usize>() > 96 * 1024 {
        return Ok(125);
    }
    channel.write_all(&(arguments.len() as u32).to_be_bytes())?;
    for argument in arguments {
        channel.write_all(&(argument.len() as u32).to_be_bytes())?;
        channel.write_all(argument.as_bytes())?;
    }
    let mut status = [0; 4];
    channel.read_exact(&mut status)?;
    let status = i32::from_be_bytes(status);
    if status != 0 {
        return Ok(status);
    }
    let mut input = channel.try_clone()?;
    let producer = std::thread::spawn(move || {
        let result = io::copy(&mut io::stdin().lock(), &mut input);
        let _ = input.shutdown(Shutdown::Write);
        result
    });
    // A binary READY frame may carry no newline. Forward it before EOF.
    let mut output = io::stdout().lock();
    let mut bytes = [0; 4096];
    loop {
        let count = channel.read(&mut bytes)?;
        if count == 0 {
            break;
        }
        output.write_all(&bytes[..count])?;
        output.flush()?;
    }
    producer
        .join()
        .map_err(|_| io::Error::other("fixture stdin owner failed"))??;
    Ok(0)
}
fn main() {
    std::process::exit(run().unwrap_or(125));
}
