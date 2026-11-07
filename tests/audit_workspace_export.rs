//! Worker archives are untrusted data. These regressions inspect exported bytes
//! and modes only: no Git, editor, shell, engine, or worker payload is executed.
#![cfg(any(target_os = "linux", target_os = "macos"))]

use agentsandbox::workspace::{Limits, Manifest, extract_workspace};
use sha2::{Digest, Sha256};
use std::{fs, io::Cursor, os::unix::fs::PermissionsExt, path::PathBuf};

fn entry(builder: &mut tar::Builder<Vec<u8>>, path: &str, mode: u32, data: &[u8]) {
    let mut header = tar::Header::new_gnu();
    header.set_size(data.len() as u64);
    header.set_mode(mode);
    header.set_entry_type(tar::EntryType::Regular);
    builder.append_data(&mut header, path, data).unwrap();
}

fn export(bytes: Vec<u8>, limits: Limits) -> (tempfile::TempDir, PathBuf, Manifest) {
    let original = tempfile::tempdir().unwrap();
    fs::write(original.path().join("original.txt"), b"unchanged").unwrap();
    let parent = tempfile::tempdir().unwrap();
    let destination = parent.path().join("export");
    let manifest =
        extract_workspace(Cursor::new(bytes), &destination, original.path(), limits).unwrap();
    assert_eq!(fs::read_dir(original.path()).unwrap().count(), 1);
    assert_eq!(
        fs::read(original.path().join("original.txt")).unwrap(),
        b"unchanged"
    );
    (parent, destination, manifest)
}

fn assert_refused_without_publication(bytes: Vec<u8>, limits: Limits) {
    let original = tempfile::tempdir().unwrap();
    let parent = tempfile::tempdir().unwrap();
    let destination = parent.path().join("export");
    assert!(extract_workspace(Cursor::new(bytes), &destination, original.path(), limits).is_err());
    assert!(!destination.exists());
    assert_eq!(fs::read_dir(parent.path()).unwrap().count(), 0);
}

/// Inverted oracle from audit_hunt_main_files_workspace_export.rs: the old
/// implementation recreates both .git/config and an executable pre-commit hook.
#[test]
fn export_omits_worker_git_metadata_and_executable_hook() {
    let mut builder = tar::Builder::new(Vec::new());
    entry(&mut builder, "./README.md", 0o644, b"project");
    entry(
        &mut builder,
        "./.git/config",
        0o644,
        b"[core]\n\tfsmonitor = fixture-command\n",
    );
    entry(
        &mut builder,
        "./.git/hooks/pre-commit",
        0o755,
        b"inert fixture bytes; never execute",
    );
    let (_parent, destination, manifest) = export(builder.into_inner().unwrap(), Limits::default());
    assert!(!destination.join(".git").exists());
    assert_eq!(
        manifest.keys().map(String::as_str).collect::<Vec<_>>(),
        ["README.md"]
    );
    assert_eq!(fs::read(destination.join("README.md")).unwrap(), b"project");
    assert_eq!(
        manifest["README.md"].sha256,
        hex::encode(Sha256::digest(b"project"))
    );
}

#[test]
fn export_files_default_to_0644_without_preserving_worker_execute_bits() {
    for mode in [0o000, 0o644, 0o700, 0o755, 0o777, 0o4755] {
        let mut builder = tar::Builder::new(Vec::new());
        entry(&mut builder, "nested/tool", mode, b"ordinary source bytes");
        let (_parent, destination, manifest) =
            export(builder.into_inner().unwrap(), Limits::default());
        assert_eq!(manifest.len(), 1);
        let output = destination.join("nested/tool");
        assert_eq!(fs::read(&output).unwrap(), b"ordinary source bytes");
        assert_eq!(
            fs::metadata(output).unwrap().permissions().mode() & 0o7777,
            0o644
        );
    }
}

#[test]
fn export_filters_reserved_components_and_tool_metadata_case_insensitively() {
    let excluded = [
        ".git/config",
        ".SSH/key",
        ".aws/config",
        ".azure/config",
        ".gnupg/data",
        ".kube/config",
        ".agentsandbox/session",
        "Secrets/value",
        "mounted-secrets/value",
        "credential-snapshots/value",
        "credentials/value",
        "node_modules/pkg/source",
        "target/debug/output",
        ".venv/bin/tool",
        "venv/bin/tool",
        ".npmrc",
        ".pypirc",
        ".netrc",
        "id_rsa",
        "ID_ED25519",
        "id_ecdsa",
        ".DS_STORE",
        ".ENV",
        ".Env.Production",
        "credentials.json",
        "server.PEM",
        "server.key",
        "client.p12",
        "client.pfx",
        "client.keystore",
        "nested/.GiT/config",
        "nested/.HUSKY/pre-commit",
        "nested/.ENVRC",
        "nested/.IDEA/workspace.xml",
        "nested/.VSCODE/TASKS.JSON",
        "nested/.vscode/settings.json",
        "nested/.vscode/launch.json",
        "nested/.devcontainer/devcontainer.json",
        "nested/.direnv/activation",
        ".pre-commit-config.yaml",
        ".gitattributes",
        ".gitmodules",
        ".mise.toml",
        ".tool-versions",
    ];
    let ordinary = [
        "README.md",
        "nested/.vscode-control/settings.json",
        "nested/tasks.json",
        "envrc.txt",
    ];
    let mut builder = tar::Builder::new(Vec::new());
    for path in [".git", ".HUSKY", ".IDEA", ".ENVRC"] {
        let mut header = tar::Header::new_gnu();
        header.set_entry_type(tar::EntryType::Directory);
        header.set_size(0);
        header.set_mode(0o755);
        builder.append_data(&mut header, path, &[][..]).unwrap();
    }
    for path in excluded {
        entry(&mut builder, path, 0o755, b"private fixture bytes");
    }
    for path in ordinary {
        entry(&mut builder, path, 0o644, b"ordinary");
    }
    let (_parent, destination, manifest) = export(builder.into_inner().unwrap(), Limits::default());
    assert_eq!(manifest.len(), ordinary.len());
    for path in [".git", ".HUSKY", ".IDEA", ".ENVRC"] {
        assert!(!destination.join(path).exists());
    }
    for path in excluded {
        assert!(!manifest.contains_key(path));
        assert!(!destination.join(path).exists());
    }
    for path in ordinary {
        assert_eq!(fs::read(destination.join(path)).unwrap(), b"ordinary");
        assert_eq!(manifest[path].bytes, 8);
    }
}

#[test]
fn effective_pax_and_gnu_long_paths_cannot_bypass_export_filter() {
    let long = format!("{}/.GiT/config", "d".repeat(110));
    let mut builder = tar::Builder::new(Vec::new());
    entry(&mut builder, &long, 0o755, b"inert");
    for path in ["./.git/config", "nested/.VSCode/tasks.JSON"] {
        builder
            .append_pax_extensions([("path", path.as_bytes())])
            .unwrap();
        entry(&mut builder, "ordinary-placeholder", 0o755, b"inert");
    }
    entry(&mut builder, "kept.txt", 0o644, b"kept");
    let (_parent, destination, manifest) = export(builder.into_inner().unwrap(), Limits::default());
    assert_eq!(
        manifest.keys().map(String::as_str).collect::<Vec<_>>(),
        ["kept.txt"]
    );
    assert_eq!(fs::read_dir(destination).unwrap().count(), 1);
}

#[test]
fn filtered_entries_still_consume_entry_byte_and_directory_limits() {
    let mut builder = tar::Builder::new(Vec::new());
    entry(&mut builder, ".git/config", 0o644, b"123456789");
    assert_refused_without_publication(
        builder.into_inner().unwrap(),
        Limits {
            max_file_bytes: 8,
            ..Limits::default()
        },
    );

    let mut builder = tar::Builder::new(Vec::new());
    entry(&mut builder, "kept", 0o644, b"1234");
    entry(&mut builder, ".git/config", 0o644, b"123456789");
    assert_refused_without_publication(
        builder.into_inner().unwrap(),
        Limits {
            max_bytes: 12,
            ..Limits::default()
        },
    );

    let mut builder = tar::Builder::new(Vec::new());
    entry(&mut builder, ".git/config", 0o644, b"inert");
    entry(&mut builder, ".git/HEAD", 0o644, b"inert");
    assert_refused_without_publication(
        builder.into_inner().unwrap(),
        Limits {
            max_files: 1,
            ..Limits::default()
        },
    );

    let mut builder = tar::Builder::new(Vec::new());
    entry(&mut builder, ".git/a/b/c", 0o644, b"inert");
    assert_refused_without_publication(
        builder.into_inner().unwrap(),
        Limits {
            max_files: 2,
            ..Limits::default()
        },
    );
}

#[test]
fn filtered_entries_do_not_bypass_link_path_or_duplicate_validation() {
    for kind in *b"12346S" {
        let mut builder = tar::Builder::new(Vec::new());
        let mut header = tar::Header::new_gnu();
        header.set_entry_type(tar::EntryType::new(kind));
        header.set_size(0);
        header.set_mode(0o755);
        builder
            .append_data(&mut header, ".git/hook", &[][..])
            .unwrap();
        assert_refused_without_publication(builder.into_inner().unwrap(), Limits::default());
    }
    for path in [".git/../escape", ".git\\config", ".git/config:alias"] {
        let mut builder = tar::Builder::new(Vec::new());
        let mut header = tar::Header::new_gnu();
        header.as_mut_bytes()[..path.len()].copy_from_slice(path.as_bytes());
        header.set_entry_type(tar::EntryType::Regular);
        header.set_size(0);
        header.set_mode(0o644);
        header.set_cksum();
        builder.append(&header, &[][..]).unwrap();
        assert_refused_without_publication(builder.into_inner().unwrap(), Limits::default());
    }
    let mut builder = tar::Builder::new(Vec::new());
    entry(&mut builder, ".git/config", 0o644, b"one");
    entry(&mut builder, "./.git/config", 0o644, b"two");
    assert_refused_without_publication(builder.into_inner().unwrap(), Limits::default());
}
