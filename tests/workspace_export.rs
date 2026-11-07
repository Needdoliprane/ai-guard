//! Lot J regressions derived from audit_cmp_export_git.rs on 5e52807.
//! Archive bytes are synthetic and inert. Inspect only paths, data and modes;
//! never invoke Git, an editor, a shell or any exported file. Run in the QA
//! sandbox with temporary files confined to /scratch.
#![cfg(any(target_os = "linux", target_os = "macos"))]

use agentsandbox::workspace::{Limits, extract_workspace, extract_workspace_with_report};
use std::{fs, io::Cursor, os::unix::fs::PermissionsExt, path::Path};

fn file(builder: &mut tar::Builder<Vec<u8>>, path: &str, mode: u32, data: &[u8]) {
    let mut header = tar::Header::new_gnu();
    header.set_size(data.len() as u64);
    header.set_mode(mode);
    header.set_entry_type(tar::EntryType::Regular);
    builder.append_data(&mut header, path, data).unwrap();
}

fn directory(builder: &mut tar::Builder<Vec<u8>>, path: &str) {
    let mut header = tar::Header::new_gnu();
    header.set_size(0);
    header.set_mode(0o755);
    header.set_entry_type(tar::EntryType::Directory);
    builder.append_data(&mut header, path, &[][..]).unwrap();
}

fn bare_layout(builder: &mut tar::Builder<Vec<u8>>, prefix: &str) {
    file(
        builder,
        &format!("{prefix}HEAD"),
        0o644,
        b"ref: refs/heads/main\n",
    );
    file(
        builder,
        &format!("{prefix}config"),
        0o644,
        b"[core]\n\trepositoryformatversion = 0\n\tbare = false\n\tworktree = .\n[aiguard]\n\tmarker = synthetic-worker-config\n",
    );
    directory(builder, &format!("{prefix}objects"));
    directory(builder, &format!("{prefix}refs"));
}

fn assert_refused_without_publication(bytes: Vec<u8>) {
    let original = tempfile::tempdir().unwrap();
    let parent = tempfile::tempdir().unwrap();
    let destination = parent.path().join("result");
    let result = extract_workspace(
        Cursor::new(bytes),
        &destination,
        original.path(),
        Limits::default(),
    );
    assert!(
        result.is_err(),
        "worker-authored Git directory must be refused"
    );
    assert!(!destination.exists(), "refusal must precede publication");
    assert_eq!(fs::read_dir(parent.path()).unwrap().count(), 0);
}

#[test]
fn export_refuses_bare_git_layout_at_root_before_publication() {
    let mut builder = tar::Builder::new(Vec::new());
    file(&mut builder, "README.md", 0o644, b"ordinary source");
    bare_layout(&mut builder, "");
    assert_refused_without_publication(builder.into_inner().unwrap());
}

#[test]
fn export_refuses_nested_bare_git_layout_with_late_directory_headers() {
    let mut builder = tar::Builder::new(Vec::new());
    // The HEAD/config files precede the directory headers, as legal archives
    // may do. A one-pass policy cannot rely on the header ordering.
    bare_layout(&mut builder, "nested/repository/");
    assert_refused_without_publication(builder.into_inner().unwrap());
}

#[test]
fn export_refuses_bare_git_layout_with_implicit_directories_and_late_head() {
    let mut builder = tar::Builder::new(Vec::new());
    file(&mut builder, "nested/objects/aa/object", 0o644, b"inert");
    file(&mut builder, "nested/refs/heads/main", 0o644, b"inert");
    file(
        &mut builder,
        "nested/HEAD",
        0o644,
        b"ref: refs/heads/main\n",
    );
    assert_refused_without_publication(builder.into_inner().unwrap());
}

#[test]
fn export_refuses_head_with_a_common_directory_pointer() {
    let mut builder = tar::Builder::new(Vec::new());
    file(
        &mut builder,
        "nested/HEAD",
        0o644,
        b"ref: refs/heads/main\n",
    );
    file(&mut builder, "nested/commondir", 0o644, b"../elsewhere\n");
    assert_refused_without_publication(builder.into_inner().unwrap());
}

#[test]
fn export_omits_unicode_aliases_of_blocked_tool_metadata() {
    let mut builder = tar::Builder::new(Vec::new());
    for path in [
        ".v\u{17f}code/ta\u{17f}ks.json",
        ".hu\u{17f}ky/pre-commit",
        ".vscode/tas\u{212a}s.json",
        ".g\u{200d}it/config",
    ] {
        file(&mut builder, path, 0o755, b"inert fixture; never execute");
    }
    for path in [
        "docs/caf\u{e9}.txt",
        "docs/\u{65e5}\u{672c}\u{8a9e}.txt",
        ".vscode-control/tasks.json",
    ] {
        file(&mut builder, path, 0o644, b"ordinary");
    }
    let original = tempfile::tempdir().unwrap();
    let parent = tempfile::tempdir().unwrap();
    let destination = parent.path().join("result");
    let manifest = extract_workspace(
        Cursor::new(builder.into_inner().unwrap()),
        &destination,
        original.path(),
        Limits::default(),
    )
    .unwrap();
    for path in [
        ".v\u{17f}code/ta\u{17f}ks.json",
        ".hu\u{17f}ky/pre-commit",
        ".vscode/tas\u{212a}s.json",
        ".g\u{200d}it/config",
    ] {
        assert!(
            !manifest.contains_key(path),
            "blocked Unicode alias: {path}"
        );
        assert!(!destination.join(path).exists());
    }
    for path in [
        "docs/caf\u{e9}.txt",
        "docs/\u{65e5}\u{672c}\u{8a9e}.txt",
        ".vscode-control/tasks.json",
    ] {
        assert_eq!(fs::read(destination.join(path)).unwrap(), b"ordinary");
        assert!(manifest.contains_key(path));
    }
}

#[test]
fn export_report_counts_explicit_omitted_entries_without_counting_implicit_parents() {
    let mut builder = tar::Builder::new(Vec::new());
    directory(&mut builder, ".git");
    file(&mut builder, ".git/config", 0o644, b"inert");
    file(&mut builder, ".vscode/tasks.json", 0o644, b"inert");
    file(&mut builder, "docs/kept.txt", 0o755, b"ordinary");
    directory(&mut builder, "empty");
    let original = tempfile::tempdir().unwrap();
    let parent = tempfile::tempdir().unwrap();
    let destination = parent.path().join("result");
    let report = extract_workspace_with_report(
        Cursor::new(builder.into_inner().unwrap()),
        &destination,
        original.path(),
        Limits::default(),
    )
    .unwrap();
    assert_eq!(report.omitted_entries, 3_u64);
    assert_eq!(report.manifest.len(), 1);
    assert_eq!(report.manifest["docs/kept.txt"].mode, Some(0o644));
    assert_eq!(
        fs::read(destination.join("docs/kept.txt")).unwrap(),
        b"ordinary"
    );
    assert!(destination.join("empty").is_dir());
    assert!(!destination.join(".git").exists());
    assert!(!destination.join(".vscode").exists());
}

#[test]
fn export_keeps_arbitrary_head_and_config_without_git_directory_shape() {
    for missing in ["HEAD", "objects", "refs"] {
        let mut builder = tar::Builder::new(Vec::new());
        if missing != "HEAD" {
            file(&mut builder, "HEAD", 0o644, b"ordinary heading\n");
        }
        file(&mut builder, "config", 0o644, b"ordinary configuration\n");
        for path in ["objects", "refs"] {
            if path != missing {
                directory(&mut builder, path);
            }
        }
        let original = tempfile::tempdir().unwrap();
        let parent = tempfile::tempdir().unwrap();
        let destination = parent.path().join("result");
        let manifest = extract_workspace(
            Cursor::new(builder.into_inner().unwrap()),
            &destination,
            original.path(),
            Limits::default(),
        )
        .unwrap();
        assert!(manifest.contains_key("config"));
        assert_eq!(
            fs::read(destination.join("config")).unwrap(),
            b"ordinary configuration\n"
        );
        if missing != "HEAD" {
            assert!(manifest.contains_key("HEAD"));
        }
    }
}

#[test]
fn export_removes_all_executable_and_special_mode_bits() {
    for mode in [0o755, 0o4755, 0o6777, 0o1777, 0o111] {
        let mut builder = tar::Builder::new(Vec::new());
        file(
            &mut builder,
            "bin/tool",
            mode,
            b"inert fixture; never execute",
        );
        let original = tempfile::tempdir().unwrap();
        let parent = tempfile::tempdir().unwrap();
        let destination = parent.path().join("result");
        extract_workspace(
            Cursor::new(builder.into_inner().unwrap()),
            &destination,
            original.path(),
            Limits::default(),
        )
        .unwrap();
        assert_eq!(mode_of(&destination.join("bin/tool")), 0o644);
    }
}

fn mode_of(path: &Path) -> u32 {
    fs::metadata(path).unwrap().permissions().mode() & 0o7777
}
