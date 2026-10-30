use agentsandbox::{audit::Store, model::Event, redaction::Redactor};
use std::{fs::OpenOptions, io::Write, os::unix::fs::OpenOptionsExt, path::Path};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let path = Path::new("/tmp/ai-guard-sqlite-two-uid-8qmtsx/audit.sqlite");
    let store = Store::open(path, Redactor::new(vec![]), 100)?;
    store.append(Event::new("linux.private.fixture"))?;
    let mut public = OpenOptions::new().write(true).create_new(true).mode(0o644)
        .open("/tmp/ai-guard-sqlite-readable-8qmtsx")?;
    public.write_all(b"public fixture control\n")?;
    assert!(store.integrity()?.valid);
    for suffix in ["", "-wal", "-shm"] {
        let name = format!("{}{suffix}", path.display());
        assert!(std::fs::metadata(name)?.is_file());
    }
    println!("READY: real Store, fixture event and live main/WAL/SHM");
    std::io::stdout().flush()?;
    std::thread::sleep(std::time::Duration::from_secs(600));
    drop(store);
    Ok(())
}
