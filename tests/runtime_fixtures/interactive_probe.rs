//! Deliberately small worker-side probe for the real interactive Docker test.
//! It has no provider integration, credentials, or network behavior.
use std::{
    io::{self, IsTerminal, Read, Write},
    process::{Command, Stdio},
    thread,
    time::Duration,
};

fn terminal_size_until(expected: &str) -> Result<String, Box<dyn std::error::Error>> {
    let deadline = std::time::Instant::now() + Duration::from_secs(8);
    loop {
        let output = Command::new("/usr/bin/stty")
            .arg("size")
            .stdin(Stdio::inherit())
            .output()?;
        if !output.status.success() {
            return Err("stty size failed".into());
        }
        let actual = String::from_utf8(output.stdout)?.trim().to_owned();
        if actual == expected {
            return Ok(actual);
        }
        if std::time::Instant::now() >= deadline {
            return Err("terminal resize was not observed before deadline".into());
        }
        thread::sleep(Duration::from_millis(25));
    }
}

fn run_raw_keys() -> Result<(), Box<dyn std::error::Error>> {
    let saved = Command::new("/usr/bin/stty")
        .arg("-g")
        .stdin(Stdio::inherit())
        .output()?;
    if !saved.status.success() {
        return Err("could not save remote terminal mode".into());
    }
    let saved = String::from_utf8(saved.stdout)?.trim().to_owned();
    if saved.is_empty() {
        return Err("remote terminal mode was empty".into());
    }
    let raw = Command::new("/usr/bin/stty")
        .args(["raw", "-echo"])
        .stdin(Stdio::inherit())
        .status()?;
    if !raw.success() {
        return Err("could not put remote terminal in raw mode".into());
    }
    println!("RAW_KEYS_READY");
    io::stdout().flush()?;

    // Public, fixed test bytes: CR, LF, Ctrl-C, UTF-8 é, and cursor-up.
    let mut payload = [0u8; 8];
    let read_result = io::stdin().read_exact(&mut payload);
    let restore = Command::new("/usr/bin/stty")
        .arg(&saved)
        .stdin(Stdio::inherit())
        .status()?;
    if !restore.success() {
        return Err("could not restore remote terminal mode".into());
    }
    read_result?;
    print!("RAW_KEYS_RECEIVED:");
    for (index, byte) in payload.iter().enumerate() {
        if index > 0 {
            print!(",");
        }
        print!("{byte:02x}");
    }
    println!();
    println!("RAW_KEYS_DONE");
    io::stdout().flush()?;
    Ok(())
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mode = std::env::args().nth(1).ok_or("probe mode required")?;
    if !io::stdin().is_terminal() || !io::stdout().is_terminal() || !io::stderr().is_terminal() {
        eprintln!("probe standard streams are not all terminals");
        std::process::exit(70);
    }
    match mode.as_str() {
        "exit7" => {
            let initial_size = terminal_size_until("24 80")?;
            println!("PROBE_INITIAL_SIZE:{initial_size}");
            println!("PROBE_READY");
            io::stdout().flush()?;
            let mut line = String::new();
            io::stdin().read_line(&mut line)?;
            let size = terminal_size_until("40 120")?;
            print!("PROBE_ACK:{}:{}", line.trim(), size);
            io::stdout().flush()?;
            io::stdout().write_all(b"\nSAFE_BEFORE\n\x1b]52;c;bm90LWFjdGl2ZQ==\x07\x1bP1;2|unsafe\x1b\\\x1b]0;forbidden-title\x07\x1b[5i\nSAFE_AFTER\n")?;
            io::stdout().flush()?;
            let mut finish = String::new();
            io::stdin().read_line(&mut finish)?;
            if finish.trim() != "exit" {
                return Err("explicit probe completion acknowledgement missing".into());
            }
            std::process::exit(7);
        }
        "wait" => {
            println!("STOP_READY");
            io::stdout().flush()?;
            loop {
                thread::sleep(Duration::from_secs(1));
            }
        }
        "raw-keys" => run_raw_keys(),
        _ => Err(format!("unknown probe mode: {mode}").into()),
    }
}
