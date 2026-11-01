use super::*;
use std::{
    fs,
    os::unix::fs::{PermissionsExt, symlink},
    process::Stdio,
};
thread_local! {static FAIL_AFTER_MANIFEST:std::cell::Cell<bool>=const{std::cell::Cell::new(false)};}
pub(super) fn after_manifest() -> Result<()> {
    if FAIL_AFTER_MANIFEST.with(|flag| flag.replace(false)) {
        Err(Failure::Storage)
    } else {
        Ok(())
    }
}

fn private() -> tempfile::TempDir {
    tempfile::Builder::new()
        .permissions(fs::Permissions::from_mode(0o700))
        .tempdir()
        .unwrap()
}
async fn openssl(directory: &Path, args: &[&str]) {
    let program =
        crate::control_fs::TrustedExecutable::open(Path::new("/usr/bin/openssl")).unwrap();
    program.revalidate().unwrap();
    let mut child = tokio::process::Command::new(program.path())
        .args(args)
        .env_clear()
        .current_dir(directory)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .kill_on_drop(true)
        .spawn()
        .unwrap();
    assert!(
        tokio::time::timeout(Duration::from_secs(20), child.wait())
            .await
            .unwrap()
            .unwrap()
            .success(),
        "ephemeral product TLS fixture generation failed"
    );
}
async fn fixture() -> tempfile::TempDir {
    let directory = private();
    fs::write(directory.path().join("pki.cnf"),"[req]\nprompt=no\ndistinguished_name=subject\n[subject]\nCN=TLS fixture\n[root]\nbasicConstraints=critical,CA:true\nkeyUsage=critical,keyCertSign,cRLSign\nsubjectKeyIdentifier=hash\n[server]\nbasicConstraints=critical,CA:false\nkeyUsage=critical,digitalSignature,keyEncipherment\nextendedKeyUsage=serverAuth\nsubjectAltName=DNS:llm.ai-guard.test\n").unwrap();
    openssl(
        directory.path(),
        &[
            "req",
            "-x509",
            "-newkey",
            "rsa:2048",
            "-nodes",
            "-sha256",
            "-days",
            "7",
            "-config",
            "pki.cnf",
            "-extensions",
            "root",
            "-keyout",
            "ca.key",
            "-out",
            "ca.pem",
        ],
    )
    .await;
    openssl(
        directory.path(),
        &[
            "req",
            "-new",
            "-newkey",
            "rsa:2048",
            "-nodes",
            "-sha256",
            "-subj",
            "/CN=TLS fixture leaf",
            "-keyout",
            "server.key",
            "-out",
            "server.csr",
        ],
    )
    .await;
    openssl(
        directory.path(),
        &[
            "x509",
            "-req",
            "-in",
            "server.csr",
            "-CA",
            "ca.pem",
            "-CAkey",
            "ca.key",
            "-set_serial",
            "101",
            "-days",
            "1",
            "-sha256",
            "-extfile",
            "pki.cnf",
            "-extensions",
            "server",
            "-out",
            "server.pem",
        ],
    )
    .await;
    for file in ["ca.pem", "ca.key", "server.pem", "server.key"] {
        fs::set_permissions(
            directory.path().join(file),
            fs::Permissions::from_mode(0o600),
        )
        .unwrap();
    }
    directory
}
fn packet(directory: &Path) -> PacketV1 {
    PacketV1 {
        version: 1,
        generation: Uuid::new_v4().to_string(),
        server_name: "llm.ai-guard.test".into(),
        ca_pem: fs::read_to_string(directory.join("ca.pem")).unwrap(),
        cert_pem: fs::read_to_string(directory.join("server.pem")).unwrap(),
        key_pem: fs::read_to_string(directory.join("server.key")).unwrap(),
    }
}
fn copied(packet: &PacketV1) -> PacketV1 {
    decode_packet(&serde_json::to_vec(packet).unwrap()).unwrap()
}

async fn dated_leaf(
    directory: &Path,
    label: &str,
    start: chrono::DateTime<chrono::Utc>,
    end: chrono::DateTime<chrono::Utc>,
) -> String {
    let config = format!(
        "[ca]\ndefault_ca=local\n[local]\ndatabase=index-{label}\nserial=serial-{label}\nnew_certs_dir=.\ncertificate=ca.pem\nprivate_key=ca.key\ndefault_md=sha256\npolicy=subject\nx509_extensions=server\n[subject]\ncommonName=supplied\n[server]\nbasicConstraints=critical,CA:false\nkeyUsage=critical,digitalSignature,keyEncipherment\nextendedKeyUsage=serverAuth\nsubjectAltName=DNS:llm.ai-guard.test\n"
    );
    let config_name = format!("dated-{label}.cnf");
    let output = format!("dated-{label}.pem");
    fs::write(directory.join(&config_name), config).unwrap();
    fs::write(directory.join(format!("index-{label}")), b"").unwrap();
    fs::write(
        directory.join(format!("serial-{label}")),
        format!("{}\n", Uuid::new_v4().simple()),
    )
    .unwrap();
    openssl(
        directory,
        &[
            "ca",
            "-batch",
            "-notext",
            "-config",
            &config_name,
            "-in",
            "server.csr",
            "-out",
            &output,
            "-startdate",
            &start.format("%Y%m%d%H%M%SZ").to_string(),
            "-enddate",
            &end.format("%Y%m%d%H%M%SZ").to_string(),
        ],
    )
    .await;
    fs::read_to_string(directory.join(output)).unwrap()
}

#[test]
fn packet_and_ack_are_closed_bounded_and_require_one_complete_document() {
    let body=br#"{"version":1,"generation":"00000000-0000-0000-0000-000000000001","server_name":"llm.ai-guard.test","ca_pem":"fixture","cert_pem":"fixture","key_pem":"fixture"}"#;
    assert!(decode_packet(body).is_ok());
    let complete = std::str::from_utf8(body).unwrap();
    let duplicate = complete.replacen("{", "{\"version\":1,", 1);
    let unknown = complete.replacen("{", "{\"unknown\":\"fixture\",", 1);
    for bytes in [
        body.iter()
            .copied()
            .chain(b"{}".iter().copied())
            .collect::<Vec<_>>(),
        duplicate.into_bytes(),
        unknown.into_bytes(),
        body[..body.len() - 1].to_vec(),
        vec![b' '; MAX_WIRE + 1],
    ] {
        assert!(decode_packet(&bytes).is_err());
    }
    let ack = AckV1 {
        version: 1,
        generation: Uuid::new_v4().to_string(),
        server_name: "llm.ai-guard.test".into(),
        ca_certificates_sha256: vec!["a".repeat(64)],
        server_certificates_sha256: vec!["b".repeat(64)],
    };
    let bytes = serde_json::to_vec(&ack).unwrap();
    assert!(decode_ack(&bytes, &ack).is_ok());
    let mut changed = ack.clone();
    changed.generation = Uuid::new_v4().to_string();
    assert!(decode_ack(&bytes, &changed).is_err());
    assert!(decode_ack(&[bytes.clone(), b"{}".to_vec()].concat(), &ack).is_err());
    assert!(decode_ack(&vec![b' '; 4097], &ack).is_err());
    let duplicate = String::from_utf8(bytes)
        .unwrap()
        .replacen("{", "{\"version\":1,", 1);
    assert!(decode_ack(duplicate.as_bytes(), &ack).is_err());
}

#[test]
fn server_name_rejects_builtin_service_alias_collisions_and_loopback_names() {
    assert!(validate_name("llm.ai-guard.test").is_ok());
    for name in [
        "model",
        "gateway",
        "dashboard",
        "ingress",
        "local-ai-ingress",
        "localhost",
        "a.localhost",
        "127.0.0.1",
        "::1",
        "Model",
        "-invalid.test",
        "invalid..test",
    ] {
        assert!(validate_name(name).is_err());
    }
}

#[tokio::test]
async fn real_crypto_validates_identity_and_refuses_ca_key_reuse_before_writes() {
    let directory = fixture().await;
    let valid = packet(directory.path());
    let expected = validate_packet(&valid).unwrap();
    assert_eq!(expected.ca_certificates_sha256.len(), 1);
    let now = chrono::Utc::now();
    for (label, start, end) in [
        (
            "expired",
            now - chrono::Duration::days(3),
            now - chrono::Duration::days(2),
        ),
        (
            "future",
            now + chrono::Duration::days(2),
            now + chrono::Duration::days(3),
        ),
    ] {
        let mut invalid_date = copied(&valid);
        invalid_date.cert_pem = dated_leaf(directory.path(), label, start, end).await;
        assert!(validate_packet(&invalid_date).is_err());
    }
    let mut wrong_name = copied(&valid);
    wrong_name.server_name = "other.ai-guard.test".into();
    assert_eq!(
        validate_packet(&wrong_name).err(),
        Some(Failure::InvalidInput)
    );
    let mut wrong_key = copied(&valid);
    wrong_key.key_pem = fs::read_to_string(directory.path().join("ca.key")).unwrap();
    assert!(validate_packet(&wrong_key).is_err());
    // Reuse the valid server CSR/key, not the trust anchor's key. The only
    // changed validation property is the leaf's CA:true constraint.
    fs::write(directory.path().join("ca-leaf.cnf"),"[server_ca]\nbasicConstraints=critical,CA:true\nkeyUsage=critical,digitalSignature,keyEncipherment\nextendedKeyUsage=serverAuth\nsubjectAltName=DNS:llm.ai-guard.test\n").unwrap();
    openssl(
        directory.path(),
        &[
            "x509",
            "-req",
            "-in",
            "server.csr",
            "-CA",
            "ca.pem",
            "-CAkey",
            "ca.key",
            "-set_serial",
            "103",
            "-days",
            "1",
            "-sha256",
            "-extfile",
            "ca-leaf.cnf",
            "-extensions",
            "server_ca",
            "-out",
            "ca-leaf.pem",
        ],
    )
    .await;
    let mut ca_leaf = copied(&valid);
    ca_leaf.cert_pem = fs::read_to_string(directory.path().join("ca-leaf.pem")).unwrap();
    assert!(
        crate::tls::ServerIdentity::from_pem(
            ca_leaf.cert_pem.as_bytes(),
            ca_leaf.key_pem.as_bytes()
        )
        .is_ok()
    );
    let ca_chain = crate::tls::certificates(ca_leaf.cert_pem.as_bytes()).unwrap();
    let ca_parsed = ParsedCertificate::try_from(&ca_chain[0]).unwrap();
    rustls::client::verify_server_name(
        &ca_parsed,
        &ServerName::try_from("llm.ai-guard.test").unwrap(),
    )
    .unwrap();
    let anchors = crate::tls::certificates(ca_leaf.ca_pem.as_bytes()).unwrap();
    let mut ca_trust = rustls::RootCertStore::empty();
    for anchor in &anchors {
        assert!(
            ca_parsed.subject_public_key_info()
                != ParsedCertificate::try_from(anchor)
                    .unwrap()
                    .subject_public_key_info()
        );
        ca_trust.add(anchor.clone()).unwrap();
    }
    // OpenSSL's purpose-any check validates the signature, chain and dates
    // without applying rustls's server end-entity role restriction.
    openssl(
        directory.path(),
        &[
            "verify",
            "-CAfile",
            "ca.pem",
            "-purpose",
            "any",
            "ca-leaf.pem",
        ],
    )
    .await;
    let role_error = rustls::client::verify_server_cert_signed_by_trust_anchor(
        &ca_parsed,
        &ca_trust,
        &[],
        UnixTime::now(),
        rustls::crypto::ring::default_provider()
            .signature_verification_algorithms
            .all,
    )
    .unwrap_err();
    assert!(matches!(
        role_error,
        rustls::Error::InvalidCertificate(rustls::CertificateError::Other(other))
            if other.0.to_string() == "CaUsedAsEndEntity"
    ));
    assert_eq!(validate_packet(&ca_leaf).err(), Some(Failure::InvalidInput));
    openssl(
        directory.path(),
        &[
            "req",
            "-new",
            "-key",
            "ca.key",
            "-subj",
            "/CN=Reused key leaf",
            "-out",
            "reused.csr",
        ],
    )
    .await;
    openssl(
        directory.path(),
        &[
            "x509",
            "-req",
            "-in",
            "reused.csr",
            "-CA",
            "ca.pem",
            "-CAkey",
            "ca.key",
            "-set_serial",
            "102",
            "-days",
            "1",
            "-sha256",
            "-extfile",
            "pki.cnf",
            "-extensions",
            "server",
            "-out",
            "reused.pem",
        ],
    )
    .await;
    let mut reused = copied(&valid);
    reused.cert_pem = fs::read_to_string(directory.path().join("reused.pem")).unwrap();
    reused.key_pem = fs::read_to_string(directory.path().join("ca.key")).unwrap();
    // The pair is valid and the leaf is not a CA; rejection must also cover
    // reuse of the key represented by a supplied trust certificate.
    assert!(
        crate::tls::ServerIdentity::from_pem(reused.cert_pem.as_bytes(), reused.key_pem.as_bytes())
            .is_ok()
    );
    let chain = crate::tls::certificates(reused.cert_pem.as_bytes()).unwrap();
    let parsed = ParsedCertificate::try_from(&chain[0]).unwrap();
    let mut trust = rustls::RootCertStore::empty();
    for ca in crate::tls::certificates(reused.ca_pem.as_bytes()).unwrap() {
        trust.add(ca).unwrap();
    }
    rustls::client::verify_server_cert_signed_by_trust_anchor(
        &parsed,
        &trust,
        &[],
        UnixTime::now(),
        rustls::crypto::ring::default_provider()
            .signature_verification_algorithms
            .all,
    )
    .unwrap();
    rustls::client::verify_server_name(
        &parsed,
        &ServerName::try_from("llm.ai-guard.test").unwrap(),
    )
    .unwrap();
    assert!(validate_packet(&reused).is_err());
    let roots = private();
    let root = Directory::open_existing(&absolute_path(roots.path()).unwrap()).unwrap();
    let ca = root.exclusive_child("ca").unwrap();
    let identity = root.exclusive_child("identity").unwrap();
    assert!(import_into(reused, &ca, &identity).is_err());
    assert_eq!(ca.persistent_inventory(1).unwrap().visited, 0);
    assert_eq!(identity.persistent_inventory(1).unwrap().visited, 0);
    let imported = import_into(copied(&valid), &ca, &identity).unwrap();
    assert_eq!(imported, expected);
    for path in [
        ca.path().join("root-ca.pem"),
        identity.path().join("server-chain.pem"),
        identity.path().join("server.key"),
    ] {
        assert_eq!(
            fs::metadata(path).unwrap().permissions().mode() & 0o777,
            0o600
        );
    }
    assert!(import_into(copied(&valid), &ca, &identity).is_err());
}

#[tokio::test]
async fn sources_are_private_bounded_retained_and_substitution_is_not_adopted() {
    let directory = fixture().await;
    let path = directory.path();
    let ca = path.join("ca.pem");
    let cert = path.join("server.pem");
    let key = path.join("server.key");
    let sources = Sources::open(&ca, &cert, &key, "llm.ai-guard.test".into()).unwrap();
    assert!(decode_packet(&sources.wire().unwrap()).is_ok());
    fs::set_permissions(&key, fs::Permissions::from_mode(0o644)).unwrap();
    assert!(Sources::open(&ca, &cert, &key, "llm.ai-guard.test".into()).is_err());
    fs::set_permissions(&key, fs::Permissions::from_mode(0o600)).unwrap();
    let retained = Sources::open(&ca, &cert, &key, "llm.ai-guard.test".into()).unwrap();
    let alias = path.join("alias.key");
    symlink(&key, &alias).unwrap();
    assert!(Sources::open(&ca, &cert, &alias, "llm.ai-guard.test".into()).is_err());
    let replacement = path.join("replacement.key");
    fs::copy(&key, &replacement).unwrap();
    fs::set_permissions(&replacement, fs::Permissions::from_mode(0o600)).unwrap();
    fs::rename(replacement, &key).unwrap();
    assert!(retained.revalidate().is_err());
    let oversized = path.join("oversized.pem");
    fs::write(&oversized, vec![b'x'; MAX_CERT + 1]).unwrap();
    fs::set_permissions(&oversized, fs::Permissions::from_mode(0o600)).unwrap();
    assert!(Sources::open(&oversized, &cert, &key, "llm.ai-guard.test".into()).is_err());
    fs::set_permissions(path, fs::Permissions::from_mode(0o755)).unwrap();
    assert!(Sources::open(&ca, &cert, &key, "llm.ai-guard.test".into()).is_err());
    fs::set_permissions(path, fs::Permissions::from_mode(0o700)).unwrap();
}

#[test]
fn publication_prepares_both_files_and_never_overwrites_an_existing_destination() {
    let directory = private();
    let final_path = directory.path().join("selected");
    let plan = OutputPlan::prepare(&final_path).unwrap();
    assert!(!final_path.exists());
    let ack = AckV1 {
        version: 1,
        generation: Uuid::new_v4().to_string(),
        server_name: "llm.ai-guard.test".into(),
        ca_certificates_sha256: vec!["a".repeat(64)],
        server_certificates_sha256: vec!["b".repeat(64)],
    };
    let runtime = crate::runtime::model_tls::RuntimeProvenance {
        invocation_sha256: "d".repeat(64),
        authority: crate::runtime::model_tls::RuntimeAuthority::Docker {
            unix_endpoint: "unix:///fixture/engine.sock".into(),
            engine_id: "fixture-engine".into(),
        },
    };
    let volumes = VolumeSelection::new(&ack.generation);
    let image = format!("sha256:{}", "c".repeat(64));
    publish(&plan, &ack, &image, &volumes, &runtime).unwrap();
    let env = fs::read_to_string(final_path.join("volumes.env")).unwrap();
    assert_eq!(env.lines().count(), 5);
    assert!(env.contains(&volumes.network));
    let manifest = fs::read(final_path.join("manifest.json")).unwrap();
    let parsed: serde_json::Value = serde_json::from_slice(&manifest).unwrap();
    assert_eq!(
        parsed["runtime"]["invocation_sha256"],
        runtime.invocation_sha256
    );
    assert_eq!(
        parsed["runtime"]["authority"]["unix_endpoint"],
        "unix:///fixture/engine.sock"
    );
    assert_eq!(
        parsed["runtime"]["authority"]["engine_id"],
        "fixture-engine"
    );
    assert!(OutputPlan::prepare(&final_path).is_err());
    assert_eq!(
        fs::read(final_path.join("manifest.json")).unwrap(),
        manifest
    );
    let interrupted_path = directory.path().join("before-rename");
    let interrupted = OutputPlan::prepare(&interrupted_path).unwrap();
    FAIL_AFTER_MANIFEST.with(|flag| flag.set(true));
    assert_eq!(
        publish(&interrupted, &ack, &image, &volumes, &runtime).err(),
        Some(Failure::Storage)
    );
    assert!(!interrupted_path.exists());
    assert!(interrupted.staged.path().join("manifest.json").is_file());
    assert!(!interrupted.staged.path().join("volumes.env").exists());
    let unknown_path = directory.path().join("after-rename");
    let unknown = OutputPlan::prepare(&unknown_path).unwrap();
    crate::control_fs::publication::FAIL_AFTER_RENAME.with(|flag| flag.set(true));
    assert_eq!(
        publish(&unknown, &ack, &image, &volumes, &runtime).err(),
        Some(Failure::PublicationUnknown)
    );
    assert!(unknown_path.join("manifest.json").is_file());
    assert!(unknown_path.join("volumes.env").is_file());
}
