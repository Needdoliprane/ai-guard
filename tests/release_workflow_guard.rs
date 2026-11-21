//! Guards on the release workflow: the archive-member check of the draft job is extracted from
//! `.github/workflows/release.yaml` and replayed on synthetic archives (red and green), and the
//! job wiring that keeps a failed metadata step from tagging is asserted on the workflow text.
//! No GitHub call is made and nothing is built.

use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::Command;

const TAG: &str = "v0.0.1";

fn workflow(name: &str) -> String {
    fs::read_to_string(
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .join(".github/workflows")
            .join(name),
    )
    .unwrap_or_else(|error| panic!("{name}: {error}"))
}

/// The shell function `verify_archive_members` exactly as the draft job defines it.
fn member_check() -> String {
    let text = workflow("release.yaml");
    let mut lines = text.lines();
    let mut function = Vec::new();
    let mut indent = None;
    for line in lines.by_ref() {
        if line.trim() == "verify_archive_members() {" {
            indent = Some(line.len() - line.trim_start().len());
            function.push(line);
            break;
        }
    }
    let indent = indent.expect("verify_archive_members is defined in release.yaml");
    for line in lines {
        function.push(line);
        if line.len() == indent + 1 && line.trim() == "}" {
            return function.join("\n");
        }
    }
    panic!("verify_archive_members is not closed");
}

struct Member<'a> {
    path: &'a str,
    content: &'a str,
    mode: u32,
}

fn archive(parent: &Path, root: &str, members: &[Member]) -> PathBuf {
    let base = parent.join(root);
    fs::create_dir_all(&base).unwrap();
    for member in members {
        let path = base.join(member.path);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(&path, member.content).unwrap();
        fs::set_permissions(&path, fs::Permissions::from_mode(member.mode)).unwrap();
    }
    let file = parent.join(format!("{root}.tar.gz"));
    let status = Command::new("tar")
        .arg("-C")
        .arg(parent)
        .arg("-czf")
        .arg(&file)
        .arg(root)
        .status()
        .expect("tar");
    assert!(status.success());
    fs::remove_dir_all(&base).unwrap();
    file
}

fn check(archive: &Path, root: &str, target: &str) -> (bool, String) {
    let script = format!(
        "{}\nverify_archive_members \"$1\" \"$2\" \"$3\"",
        member_check()
    );
    let output = Command::new("bash")
        .args(["-c", &script, "check"])
        .arg(archive)
        .arg(root)
        .arg(target)
        .env("NATIVE_BINARIES", "agentsandbox ai-guard-provider")
        .output()
        .expect("bash");
    (
        output.status.success(),
        format!(
            "{}{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        ),
    )
}

fn native(notices: Option<&str>) -> Vec<Member<'_>> {
    let mut members = vec![
        Member {
            path: "BUILD-INFO",
            content: "tag\n",
            mode: 0o644,
        },
        Member {
            path: "LICENSE",
            content: "licence\n",
            mode: 0o644,
        },
        Member {
            path: "bin/agentsandbox",
            content: "binary\n",
            mode: 0o755,
        },
        Member {
            path: "bin/ai-guard-provider",
            content: "binary\n",
            mode: 0o755,
        },
    ];
    if let Some(content) = notices {
        members.push(Member {
            path: "THIRD-PARTY-NOTICES",
            content,
            mode: 0o644,
        });
    }
    members
}

fn metadata(with_sbom: bool) -> Vec<Member<'static>> {
    let mut members = vec![
        Member {
            path: "BUILD-INFO",
            content: "tag\n",
            mode: 0o644,
        },
        Member {
            path: "LICENSE",
            content: "licence\n",
            mode: 0o644,
        },
        Member {
            path: "manifest/release-manifest.json",
            content: "{}\n",
            mode: 0o644,
        },
        Member {
            path: "manifest/SHA256SUMS",
            content: "sums\n",
            mode: 0o644,
        },
    ];
    if with_sbom {
        members.push(Member {
            path: "manifest/sbom.cdx.json",
            content: "{}\n",
            mode: 0o644,
        });
    }
    members
}

#[test]
fn native_archive_needs_a_nonempty_third_party_notices_member() {
    let dir = tempfile::tempdir().unwrap();
    let root = format!("ai-guard-{TAG}-x86_64-unknown-linux-gnu");
    let target = "x86_64-unknown-linux-gnu".to_string();

    let green = archive(&dir.path().join("green"), &root, &native(Some("notices\n")));
    let (ok, output) = check(&green, &root, &target);
    assert!(ok, "green native archive refused: {output}");

    let missing = archive(&dir.path().join("missing"), &root, &native(None));
    let (ok, output) = check(&missing, &root, &target);
    assert!(
        !ok && output.contains("THIRD-PARTY-NOTICES"),
        "missing notices accepted: {output}"
    );

    let empty = archive(&dir.path().join("empty"), &root, &native(Some("")));
    let (ok, output) = check(&empty, &root, &target);
    assert!(
        !ok && output.contains("THIRD-PARTY-NOTICES"),
        "empty notices accepted: {output}"
    );
}

#[test]
fn metadata_archive_needs_manifest_sbom_and_sums() {
    let dir = tempfile::tempdir().unwrap();
    let root = format!("ai-guard-{TAG}-release-metadata");

    let green = archive(&dir.path().join("green"), &root, &metadata(true));
    let (ok, output) = check(&green, &root, "release-metadata");
    assert!(ok, "green metadata archive refused: {output}");

    let red = archive(&dir.path().join("red"), &root, &metadata(false));
    let (ok, output) = check(&red, &root, "release-metadata");
    assert!(
        !ok && output.contains("manifest/sbom.cdx.json"),
        "missing SBOM accepted: {output}"
    );
}

#[test]
fn frontend_archive_keeps_its_vite_attribution_requirement() {
    let dir = tempfile::tempdir().unwrap();
    let root = format!("ai-guard-{TAG}-frontend");
    let frontend = |with_vite: bool| {
        let mut members = vec![
            Member {
                path: "BUILD-INFO",
                content: "tag\n",
                mode: 0o644,
            },
            Member {
                path: "LICENSE",
                content: "licence\n",
                mode: 0o644,
            },
            Member {
                path: "index.html",
                content: "<html>\n",
                mode: 0o644,
            },
            Member {
                path: "assets/app.js",
                content: "js\n",
                mode: 0o644,
            },
        ];
        if with_vite {
            members.push(Member {
                path: "licenses/vite-LICENSE.md",
                content: "vite\n",
                mode: 0o644,
            });
        }
        members
    };
    let green = archive(&dir.path().join("green"), &root, &frontend(true));
    assert!(check(&green, &root, "frontend").0);
    let red = archive(&dir.path().join("red"), &root, &frontend(false));
    let (ok, output) = check(&red, &root, "frontend");
    assert!(
        !ok && output.contains("vite-LICENSE.md"),
        "missing Vite licence accepted: {output}"
    );
}

#[test]
fn no_tag_can_exist_before_the_metadata_job_succeeds() {
    let release = workflow("release.yaml");
    let section = |name: &str| -> String {
        let header = format!("  {name}:");
        let mut lines = release.lines().skip_while(|line| *line != header);
        let mut body = vec![lines.next().unwrap_or_else(|| panic!("job {name}"))];
        for line in lines {
            let top_level =
                line.starts_with("  ") && !line.starts_with("   ") && !line.starts_with("  #");
            if top_level {
                break;
            }
            body.push(line);
        }
        body.join("\n")
    };
    assert!(
        section("tag").contains("needs: [verify, metadata]"),
        "tag must wait for metadata"
    );
    assert!(
        section("draft").contains("needs: [verify, tag, metadata, build]"),
        "draft must wait for metadata"
    );
    let metadata = section("metadata");
    assert!(metadata.contains("release manifest") && metadata.contains("--source-only"));
    assert!(metadata.contains("third_party_notices"));
}

#[test]
fn draft_publishes_and_digest_checks_the_metadata_assets() {
    let release = workflow("release.yaml");
    for needle in [
        "ai-guard-$RELEASE_TAG-release-metadata.tar.gz",
        "ai-guard-$RELEASE_TAG-sbom.cdx.json",
        "files=(\"${archives[@]}\" \"$sbom\" SHA256SUMS)",
        "shasum -a 256 \"${archives[@]}\" \"$sbom\" > SHA256SUMS",
        "run=%s\\nrepository=%s",
    ] {
        assert!(release.contains(needle), "release.yaml lost: {needle}");
    }
}

#[test]
fn ci_guards_the_reviewed_licence_list() {
    let ci = workflow("ci.yaml");
    assert!(ci.contains(
        "third_party_notices -- check --allowed .github/third-party-license-expressions.txt"
    ));
    let list =
        Path::new(env!("CARGO_MANIFEST_DIR")).join(".github/third-party-license-expressions.txt");
    let text = fs::read_to_string(list).expect("reviewed licence list is committed");
    assert!(text.lines().any(|line| line.trim() == "MIT OR Apache-2.0"));
}

#[test]
fn native_binary_list_comes_from_cargo_and_is_not_hard_coded() {
    let release = workflow("release.yaml");
    assert!(
        release.contains("select(.kind | index(\"bin\"))"),
        "binary list must come from cargo metadata"
    );
    let manifest =
        fs::read_to_string(Path::new(env!("CARGO_MANIFEST_DIR")).join("Cargo.toml")).unwrap();
    for name in [
        "ai-guard-provider",
        "ai-guard-relay",
        "ai-guard-diagnose",
        "ai-guard-activity",
    ] {
        assert!(
            !release.contains(&format!("bin/{name}")),
            "{name} must not be named in release.yaml: the list is derived from Cargo"
        );
    }
    assert!(manifest.contains("agentsandbox"));
}
