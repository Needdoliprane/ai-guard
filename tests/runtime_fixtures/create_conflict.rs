//! A deliberately tiny external-process fixture, compiled by runtime_races.
//! It models a failed create and a destructive rm of somebody else's container.
use std::{fs, io::Write};
fn fixture(key: &str) -> String {
    fs::read_to_string(
        std::env::current_exe()
            .unwrap()
            .with_extension("fixture")
            .join(key),
    )
    .unwrap()
}
fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let mut log = fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(fixture("log"))
        .unwrap();
    writeln!(
        log,
        "{}",
        args.first().map(String::as_str).unwrap_or("missing")
    )
    .unwrap();
    match args.first().map(String::as_str) {
        Some("create") => {
            eprintln!("fixture: requested container name is already in use");
            std::process::exit(125);
        }
        Some("rm") => {
            fs::remove_file(fixture("marker")).unwrap();
        }
        _ => std::process::exit(126),
    }
}
