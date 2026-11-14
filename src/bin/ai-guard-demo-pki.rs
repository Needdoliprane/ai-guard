//! Local test certificates only. Never installs trust or contacts an issuer.
use agentsandbox::build_identity::{Component, early_exit};
use agentsandbox::control_fs::{Directory, TrustedExecutable, absolute_path};
use anyhow::{Context, Result, ensure};
use clap::Parser;
use std::{
    collections::BTreeSet,
    fs::{self, File, OpenOptions},
    io::Write,
    os::unix::fs::{DirBuilderExt, MetadataExt, OpenOptionsExt},
    path::{Path, PathBuf},
    process::Stdio,
    time::Duration,
};
use tokio::{process::Command, time::Instant};
use uuid::Uuid;

const OUTPUTS: &[&str] = &[
    "pki.cnf",
    "root-ca.key",
    "root-ca.pem",
    "intermediate-ca.key",
    "intermediate-ca.csr",
    "intermediate-ca.pem",
    "server.key",
    "server.csr",
    "server.pem",
    "server-chain.pem",
];

#[derive(Parser)]
#[command(about = "Create a NEW private, local test CA/intermediate/server chain")]
struct Arguments {
    #[arg(long, default_value = ".agentsandbox/demo-pki")]
    out: PathBuf,
    /// DNS SAN; repeat up to 8. Replaces the default Keycloak/LLM/localhost names.
    #[arg(long)]
    dns: Vec<String>,
    /// Server certificate validity in days (1..=30).
    #[arg(long, default_value_t = 7)]
    days: u16,
    /// Trusted system OpenSSL/LibreSSL executable; must be an absolute path.
    #[arg(long, default_value = "/usr/bin/openssl")]
    openssl: PathBuf,
}

fn main() {
    if let Some(code) = early_exit(Component::DemoPki) {
        std::process::exit(code);
    }
    run();
}

#[tokio::main]
async fn run() {
    if let Err(error) = create_test_pki(Arguments::parse()).await {
        eprintln!("test PKI creation refused: {error:#}");
        std::process::exit(1);
    }
}

async fn create_test_pki(arguments: Arguments) -> Result<()> {
    let names = dns_names(arguments.dns)?;
    ensure!((1..=30).contains(&arguments.days), "--days must be 1..=30");
    ensure!(
        arguments.openssl.is_absolute(),
        "--openssl must be an absolute trusted executable"
    );
    let program = TrustedExecutable::open(&arguments.openssl)
        .context("trusted OpenSSL/LibreSSL executable unavailable")?;

    let path = absolute_path(&arguments.out)?;
    let parent =
        Directory::open_or_create(path.parent().context("output needs a parent directory")?)?;
    parent.revalidate()?;
    // Exclusive creation also refuses existing empty directories and symlinks.
    fs::DirBuilder::new()
        .mode(0o700)
        .create(&path)
        .context("choose a NEW output directory; existing entries are never replaced")?;
    let directory = Directory::open_existing(&path)?;
    let files: Vec<_> = OUTPUTS
        .iter()
        .map(|name| new_file(&directory, name))
        .collect::<Result<_>>()?;
    let config = configuration(&names);
    let mut config_file = &files[0];
    config_file
        .write_all(config.as_bytes())
        .context("test PKI configuration write failed")?;
    config_file
        .sync_all()
        .context("test PKI configuration sync failed")?;
    let deadline = Instant::now() + Duration::from_secs(120);
    let session = Session {
        program,
        directory,
        files,
        deadline,
    };

    let root_serial = serial();
    session
        .openssl(
            "root certificate",
            &[
                "req",
                "-x509",
                "-newkey",
                "rsa:2048",
                "-nodes",
                "-sha256",
                "-days",
                "90",
                "-config",
                "pki.cnf",
                "-extensions",
                "root_ca",
                "-subj",
                "/CN=ai-guard Local Test Root",
                "-set_serial",
                &root_serial,
                "-keyout",
                "root-ca.key",
                "-out",
                "root-ca.pem",
            ],
        )
        .await?;
    session
        .openssl(
            "intermediate request",
            &[
                "req",
                "-new",
                "-newkey",
                "rsa:2048",
                "-nodes",
                "-sha256",
                "-config",
                "pki.cnf",
                "-subj",
                "/CN=ai-guard Local Test Intermediate",
                "-keyout",
                "intermediate-ca.key",
                "-out",
                "intermediate-ca.csr",
            ],
        )
        .await?;
    let intermediate_serial = serial();
    session
        .openssl(
            "intermediate certificate",
            &[
                "x509",
                "-req",
                "-in",
                "intermediate-ca.csr",
                "-CA",
                "root-ca.pem",
                "-CAkey",
                "root-ca.key",
                "-set_serial",
                &intermediate_serial,
                "-days",
                "60",
                "-sha256",
                "-extfile",
                "pki.cnf",
                "-extensions",
                "intermediate_ca",
                "-out",
                "intermediate-ca.pem",
            ],
        )
        .await?;
    session
        .openssl(
            "server request",
            &[
                "req",
                "-new",
                "-newkey",
                "rsa:2048",
                "-nodes",
                "-sha256",
                "-config",
                "pki.cnf",
                "-keyout",
                "server.key",
                "-out",
                "server.csr",
            ],
        )
        .await?;
    let server_serial = serial();
    let days = arguments.days.to_string();
    session
        .openssl(
            "server certificate",
            &[
                "x509",
                "-req",
                "-in",
                "server.csr",
                "-CA",
                "intermediate-ca.pem",
                "-CAkey",
                "intermediate-ca.key",
                "-set_serial",
                &server_serial,
                "-days",
                &days,
                "-sha256",
                "-extfile",
                "pki.cnf",
                "-extensions",
                "server",
                "-out",
                "server.pem",
            ],
        )
        .await?;
    session
        .openssl(
            "server chain verification",
            &[
                "verify",
                "-purpose",
                "sslserver",
                "-CAfile",
                "root-ca.pem",
                "-untrusted",
                "intermediate-ca.pem",
                "server.pem",
            ],
        )
        .await?;

    let leaf = session.directory.read_private_file("server.pem", 16_384)?;
    let intermediate = session
        .directory
        .read_private_file("intermediate-ca.pem", 16_384)?;
    let mut chain = &session.files[9];
    chain
        .write_all(leaf.bytes())
        .context("server chain write failed")?;
    chain
        .write_all(intermediate.bytes())
        .context("server chain write failed")?;
    session.revalidate()?;
    for file in &session.files {
        file.sync_all().context("test PKI file sync failed")?;
    }
    sync_directory(session.directory.path())?;
    let mut ready = new_file(&session.directory, "READY")?;
    ready
        .write_all(b"local-test-pki-v1\n")
        .context("completion marker write failed")?;
    ready.sync_all().context("completion marker sync failed")?;
    session.revalidate()?;
    sync_directory(session.directory.path())?;
    parent.revalidate()?;
    sync_directory(parent.path())?;
    println!(
        "Local test PKI created. Client trust: root-ca.pem. Server: server-chain.pem + server.key.\nKeep both CA keys private. No trust store or DNS changed."
    );
    Ok(())
}

fn dns_names(mut names: Vec<String>) -> Result<Vec<String>> {
    if names.is_empty() {
        names = ["keycloak.ai-guard.test", "llm.ai-guard.test", "localhost"]
            .map(str::to_owned)
            .to_vec();
    }
    ensure!(names.len() <= 8, "at most 8 DNS SANs are supported");
    let mut seen = BTreeSet::new();
    for name in &mut names {
        name.make_ascii_lowercase();
        ensure!(
            !name.is_empty()
                && name.len() <= 253
                && name.split('.').all(|label| {
                    !label.is_empty()
                        && label.len() <= 63
                        && label
                            .bytes()
                            .all(|b| b.is_ascii_alphanumeric() || b == b'-')
                        && !label.starts_with('-')
                        && !label.ends_with('-')
                }),
            "DNS SAN must contain only valid ASCII DNS labels, without wildcards or trailing dot"
        );
        ensure!(seen.insert(name.clone()), "duplicate DNS SAN");
    }
    Ok(names)
}

fn serial() -> String {
    format!("0x{}", Uuid::new_v4().simple())
}

fn configuration(names: &[String]) -> String {
    let mut config = String::from(
        "[req]\nprompt=no\ndistinguished_name=subject\n[subject]\nCN=ai-guard Local Test Server\n[root_ca]\nbasicConstraints=critical,CA:true,pathlen:1\nkeyUsage=critical,keyCertSign,cRLSign\nsubjectKeyIdentifier=hash\n[intermediate_ca]\nbasicConstraints=critical,CA:true,pathlen:0\nkeyUsage=critical,keyCertSign,cRLSign\nsubjectKeyIdentifier=hash\nauthorityKeyIdentifier=keyid:always,issuer\n[server]\nbasicConstraints=critical,CA:false\nkeyUsage=critical,digitalSignature,keyEncipherment\nextendedKeyUsage=serverAuth\nsubjectKeyIdentifier=hash\nauthorityKeyIdentifier=keyid:always,issuer\nsubjectAltName=@names\n[names]\nIP.1=127.0.0.1\nIP.2=::1\n",
    );
    for (index, name) in names.iter().enumerate() {
        config.push_str(&format!("DNS.{}={}\n", index + 1, name));
    }
    config
}

fn new_file(directory: &Directory, name: &str) -> Result<File> {
    directory.revalidate()?;
    OpenOptions::new()
        .read(true)
        .write(true)
        .create_new(true)
        .mode(0o600)
        .custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC | libc::O_NONBLOCK)
        .open(directory.path().join(name))
        .context("private test PKI file creation refused")
}

fn sync_directory(path: &Path) -> Result<()> {
    OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_CLOEXEC)
        .open(path)
        .context("test PKI directory unavailable for sync")?
        .sync_all()
        .context("test PKI directory sync failed")
}

struct Session {
    program: TrustedExecutable,
    directory: Directory,
    files: Vec<File>,
    deadline: Instant,
}

impl Session {
    fn revalidate(&self) -> Result<()> {
        self.directory.revalidate()?;
        for (name, file) in OUTPUTS.iter().zip(&self.files) {
            let held = file
                .metadata()
                .context("test PKI retained file unavailable")?;
            let current = fs::symlink_metadata(self.directory.path().join(name))
                .context("test PKI output entry unavailable")?;
            ensure!(
                current.is_file()
                    && current.dev() == held.dev()
                    && current.ino() == held.ino()
                    && current.nlink() == 1
                    && current.mode() & 0o7777 == 0o600
                    && current.uid() == held.uid(),
                "test PKI output identity/permissions changed; partial output retained"
            );
        }
        Ok(())
    }

    async fn openssl(&self, step: &'static str, arguments: &[&str]) -> Result<()> {
        self.revalidate()?;
        self.program.revalidate()?;
        ensure!(
            Instant::now() < self.deadline,
            "test PKI generation exceeded its deadline"
        );
        let until = self.deadline.min(Instant::now() + Duration::from_secs(30));
        let mut child = Command::new(self.program.path())
            .env_clear()
            .current_dir(self.directory.path())
            .args(arguments)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .kill_on_drop(true)
            .spawn()
            .with_context(|| format!("OpenSSL/LibreSSL unavailable during {step}"))?;
        let status = match tokio::time::timeout_at(until, child.wait()).await {
            Ok(result) => result
                .with_context(|| format!("OpenSSL/LibreSSL status unavailable during {step}"))?,
            Err(_) => {
                child
                    .kill()
                    .await
                    .context("timed-out OpenSSL/LibreSSL could not be reaped")?;
                anyhow::bail!(
                    "OpenSSL/LibreSSL deadline exceeded during {step}; partial output retained"
                )
            }
        };
        self.revalidate()?;
        ensure!(
            status.success(),
            "OpenSSL/LibreSSL refused {step}; partial output retained (raw output withheld)"
        );
        Ok(())
    }
}
