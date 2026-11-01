//! Real local CA generation and TLS, without engines, providers or system trust.
#![cfg(unix)]
use agentsandbox::control_fs::TrustedExecutable;
use anyhow::{Context, Result, ensure};
use reqwest::Client;
use std::{
    error::Error,
    fs,
    os::unix::fs::{MetadataExt, PermissionsExt, symlink},
    path::Path,
    process::Stdio,
    sync::Arc,
    time::{Duration, SystemTime, UNIX_EPOCH},
};
use tempfile::TempDir;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::process::Command;
use tokio_rustls::{
    TlsAcceptor,
    rustls::{
        self,
        pki_types::{CertificateDer, PrivateKeyDer, pem::PemObject},
    },
};

fn generator(out: &Path) -> Command {
    let mut command = Command::new(env!("CARGO_BIN_EXE_ai-guard-demo-pki"));
    command
        .env_clear()
        .args(["--out"])
        .arg(out)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .kill_on_drop(true);
    command
}

fn private_fixture() -> Result<TempDir> {
    tempfile::Builder::new()
        .permissions(fs::Permissions::from_mode(0o700))
        .tempdir()
        .context("private test fixture unavailable")
}

async fn succeeds(command: &mut Command) -> Result<bool> {
    let mut child = command.spawn().context("test subprocess unavailable")?;
    let result = tokio::time::timeout(Duration::from_secs(150), child.wait()).await;
    match result {
        Ok(status) => Ok(status
            .context("test subprocess status unavailable")?
            .success()),
        Err(_) => {
            child
                .kill()
                .await
                .context("test subprocess cleanup failed")?;
            anyhow::bail!("test subprocess exceeded its deadline")
        }
    }
}

#[tokio::test]
async fn real_demo_chain_is_private_and_tls_rejects_untrusted_name_and_expiry() -> Result<()> {
    let fixture = private_fixture()?;
    let out = fixture.path().join("pki");
    ensure!(succeeds(&mut generator(&out)).await?, "generation failed");
    ensure!(fs::metadata(&out)?.mode() & 0o7777 == 0o700);
    for name in [
        "root-ca.key",
        "root-ca.pem",
        "intermediate-ca.key",
        "intermediate-ca.pem",
        "server.key",
        "server.pem",
        "server-chain.pem",
        "pki.cnf",
        "READY",
    ] {
        let facts = fs::symlink_metadata(out.join(name))?;
        ensure!(
            facts.is_file() && facts.nlink() == 1 && facts.mode() & 0o7777 == 0o600,
            "generated file permissions/type incorrect"
        );
    }
    let root_before = fs::read(out.join("root-ca.pem"))?;
    ensure!(
        !succeeds(&mut generator(&out)).await?,
        "existing output accepted"
    );
    ensure!(
        fs::read(out.join("root-ca.pem"))? == root_before,
        "existing certificate changed"
    );

    // Verify certificate expiry independently of the generating program.
    let future = (SystemTime::now().duration_since(UNIX_EPOCH)? + Duration::from_secs(31 * 86_400))
        .as_secs()
        .to_string();
    let mut expired = Command::new("/usr/bin/openssl");
    expired
        .env_clear()
        .current_dir(&out)
        .args([
            "verify",
            "-purpose",
            "sslserver",
            "-attime",
            &future,
            "-CAfile",
            "root-ca.pem",
            "-untrusted",
            "intermediate-ca.pem",
            "server.pem",
        ])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    let expired = tokio::time::timeout(Duration::from_secs(10), expired.output())
        .await
        .context("expiry verification timed out")??;
    ensure!(
        !expired.status.success()
            && expired.stdout.len() <= 8192
            && expired.stderr.len() <= 8192
            && (String::from_utf8_lossy(&expired.stdout).contains("certificate has expired")
                || String::from_utf8_lossy(&expired.stderr).contains("certificate has expired")),
        "expiry was not explicitly refused by certificate verification"
    );

    let chain = CertificateDer::pem_file_iter(out.join("server-chain.pem"))?
        .collect::<std::result::Result<Vec<_>, _>>()?;
    ensure!(
        chain.len() == 2,
        "server chain must contain leaf and intermediate"
    );
    let key = PrivateKeyDer::from_pem_file(out.join("server.key"))?;
    let config = rustls::ServerConfig::builder_with_provider(Arc::new(
        rustls::crypto::ring::default_provider(),
    ))
    .with_safe_default_protocol_versions()?
    .with_no_client_auth()
    .with_single_cert(chain, key)?;
    let acceptor = TlsAcceptor::from(Arc::new(config));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
    let address = listener.local_addr()?;
    // Real TLS on a retained loopback listener. Using the Rustls version
    // already locked by reqwest avoids s_server's platform-dependent bind flags.
    let server = tokio::spawn(async move {
        while let Ok((stream, _)) = listener.accept().await {
            let handshake =
                tokio::time::timeout(Duration::from_secs(2), acceptor.accept(stream)).await;
            if let Ok(Ok(mut stream)) = handshake {
                let exchange = async {
                    let mut request = [0u8; 4096];
                    if stream.read(&mut request).await? != 0 {
                        stream.write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 0\r\nConnection: close\r\n\r\n").await?;
                        stream.shutdown().await?;
                    }
                    Ok::<_, std::io::Error>(())
                };
                let _ = tokio::time::timeout(Duration::from_secs(2), exchange).await;
            }
        }
    });
    let result = async {
        let root = reqwest::Certificate::from_pem(&root_before)?;
        let trusted = Client::builder()
            .no_proxy()
            .https_only(true)
            .redirect(reqwest::redirect::Policy::none())
            .timeout(Duration::from_secs(5))
            .resolve("keycloak.ai-guard.test", address)
            .resolve("unapproved.ai-guard.test", address)
            .add_root_certificate(root)
            .build()?;
        let url = format!("https://keycloak.ai-guard.test:{}/", address.port());
        ensure!(
            trusted.get(&url).send().await?.status().is_success(),
            "trusted TLS failed"
        );
        let wrong_name = trusted
            .get(format!(
                "https://unapproved.ai-guard.test:{}/",
                address.port()
            ))
            .send()
            .await
            .err()
            .context("wrong SAN accepted")?;
        ensure!(
            certificate_refusal(&wrong_name, "NotValidForName"),
            "wrong SAN did not produce a certificate-name refusal"
        );
        ensure!(
            trusted.get(&url).send().await?.status().is_success(),
            "TLS server unavailable after wrong-SAN check"
        );
        let untrusted = Client::builder()
            .no_proxy()
            .https_only(true)
            .timeout(Duration::from_secs(5))
            .resolve("keycloak.ai-guard.test", address)
            .build()?;
        let unknown_ca = untrusted
            .get(&url)
            .send()
            .await
            .err()
            .context("unknown CA accepted")?;
        ensure!(
            certificate_refusal(&unknown_ca, "UnknownIssuer"),
            "unknown CA did not produce an issuer refusal"
        );
        ensure!(
            trusted.get(&url).send().await?.status().is_success(),
            "TLS server unavailable after unknown-CA check"
        );
        Ok::<_, anyhow::Error>(())
    }
    .await;
    server.abort();
    let _ = server.await;
    result
}

// The locked reqwest/rustls error chain carries these certificate reason names.
// Inspect it without logging raw errors; network outages/timeouts are not oracles.
fn certificate_refusal(error: &reqwest::Error, reason: &str) -> bool {
    if error.is_timeout() || !error.is_connect() {
        return false;
    }
    let mut source = error.source();
    while let Some(cause) = source {
        if format!("{cause:?}").contains(reason) {
            return true;
        }
        source = cause.source();
    }
    false
}

#[test]
fn trusted_executable_revalidates_replacement_and_refuses_links_or_unsafe_modes() -> Result<()> {
    let fixture = private_fixture()?;
    let executable = fixture.path().join("utility");
    fs::write(
        &executable,
        b"inert file for descriptor validation; never executed",
    )?;
    fs::set_permissions(&executable, fs::Permissions::from_mode(0o700))?;
    let held = TrustedExecutable::open(&executable)?;
    held.revalidate()?;
    fs::rename(&executable, fixture.path().join("retained"))?;
    fs::write(&executable, b"replacement, never executed")?;
    fs::set_permissions(&executable, fs::Permissions::from_mode(0o700))?;
    ensure!(
        held.revalidate().is_err(),
        "replacement executable accepted"
    );
    fs::set_permissions(&executable, fs::Permissions::from_mode(0o722))?;
    ensure!(
        TrustedExecutable::open(&executable).is_err(),
        "writable executable accepted"
    );
    fs::set_permissions(&executable, fs::Permissions::from_mode(0o600))?;
    ensure!(
        TrustedExecutable::open(&executable).is_err(),
        "non-executable accepted"
    );
    symlink(fixture.path().join("retained"), fixture.path().join("link"))?;
    ensure!(
        TrustedExecutable::open(&fixture.path().join("link")).is_err(),
        "executable link accepted"
    );
    Ok(())
}

#[tokio::test]
async fn invalid_names_and_symlink_output_are_refused_without_overwriting() -> Result<()> {
    let fixture = private_fixture()?;
    let out = fixture.path().join("pki");
    for name in [
        "good.test\nDNS.2=evil.test",
        "*.test",
        "-engine",
        "bad/host",
        "",
        "é.test",
    ] {
        ensure!(!succeeds(generator(&out).args(["--dns", name])).await?);
        ensure!(!out.exists(), "invalid SAN created output");
    }
    ensure!(!succeeds(generator(&out).args(["--days", "0"])).await?);
    ensure!(!out.exists());
    let nonprivate = fixture.path().join("nonprivate");
    fs::create_dir(&nonprivate)?;
    fs::set_permissions(&nonprivate, fs::Permissions::from_mode(0o755))?;
    let nonprivate_out = nonprivate.join("pki");
    ensure!(!succeeds(&mut generator(&nonprivate_out)).await?);
    ensure!(!nonprivate_out.exists(), "nonprivate parent created output");
    let mutable = fixture.path().join("mutable");
    fs::create_dir(&mutable)?;
    fs::set_permissions(&mutable, fs::Permissions::from_mode(0o777))?;
    let candidate = mutable.join("openssl");
    fs::write(&candidate, b"not a trusted executable")?;
    fs::set_permissions(&candidate, fs::Permissions::from_mode(0o755))?;
    ensure!(!succeeds(generator(&out).arg("--openssl").arg(&candidate)).await?);
    ensure!(!out.exists(), "unsafe executable parent created output");
    ensure!(!succeeds(generator(&out).args(["--openssl", "openssl"])).await?);
    ensure!(!out.exists());
    let outside = fixture.path().join("existing");
    fs::create_dir(&outside)?;
    fs::set_permissions(&outside, fs::Permissions::from_mode(0o700))?;
    fs::write(outside.join("keep"), b"keep")?;
    symlink(&outside, &out)?;
    ensure!(!succeeds(&mut generator(&out)).await?);
    ensure!(fs::read_dir(&outside)?.count() == 1);
    ensure!(fs::read(outside.join("keep"))? == b"keep");
    Ok(())
}
