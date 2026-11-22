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

/// The lines of one job of a workflow (two-space `jobs:` children, as these workflows are written).
fn job(text: &str, name: &str) -> Vec<String> {
    let header = format!("  {name}:");
    let mut lines = text.lines().skip_while(|line| *line != header);
    let mut body = vec![
        lines
            .next()
            .unwrap_or_else(|| panic!("job {name}"))
            .to_owned(),
    ];
    for line in lines {
        let top_level =
            line.starts_with("  ") && !line.starts_with("   ") && !line.starts_with("  #");
        if top_level || (!line.starts_with(' ') && !line.is_empty()) {
            break;
        }
        body.push(line.to_owned());
    }
    body
}

/// `needs:` of a job as a set, read only from the job's own four-space key (not from comments or steps).
fn needs(job: &[String]) -> std::collections::BTreeSet<String> {
    let line = job
        .iter()
        .find(|line| line.starts_with("    needs:"))
        .expect("job has needs");
    line.trim_start_matches("    needs:")
        .trim()
        .trim_matches(|c| c == '[' || c == ']')
        .split(',')
        .map(|name| name.trim().to_owned())
        .filter(|name| !name.is_empty())
        .collect()
}

/// The shell text of every `run:` step of a job, without shell comment lines.
fn run_blocks(job: &[String]) -> Vec<String> {
    let mut blocks = Vec::new();
    let mut index = 0;
    while index < job.len() {
        let line = &job[index];
        let trimmed = line.trim_start();
        let key = trimmed.strip_prefix("- ").unwrap_or(trimmed);
        if let Some(rest) = key.strip_prefix("run:") {
            let indent = line.len() - trimmed.len();
            let mut block = vec![rest.trim().to_owned()];
            index += 1;
            while index < job.len() {
                let next = &job[index];
                let next_indent = next.len() - next.trim_start().len();
                if !next.trim().is_empty() && next_indent <= indent + 1 {
                    break;
                }
                if !next.trim_start().starts_with('#') {
                    block.push(next.trim().to_owned());
                }
                index += 1;
            }
            blocks.push(block.join("\n"));
        } else {
            index += 1;
        }
    }
    blocks
}

fn names(items: &[&str]) -> std::collections::BTreeSet<String> {
    items.iter().map(|item| (*item).to_owned()).collect()
}

#[test]
fn no_tag_can_exist_before_the_metadata_job_succeeds() {
    let release = workflow("release.yaml");
    assert_eq!(needs(&job(&release, "tag")), names(&["verify", "metadata"]));
    assert_eq!(needs(&job(&release, "build")), names(&["verify", "tag"]));
    assert_eq!(
        needs(&job(&release, "draft")),
        names(&["verify", "tag", "metadata", "build"])
    );
    let metadata = run_blocks(&job(&release, "metadata")).join("\n");
    assert!(metadata.contains("release manifest") && metadata.contains("release verify"));
    assert!(metadata.contains("--source-only"));
}

#[test]
fn release_checks_and_packages_notices_without_missing_text_exceptions() {
    let release = workflow("release.yaml");
    let metadata = run_blocks(&job(&release, "metadata")).join("\n");
    assert!(metadata.contains("--example third_party_notices -- check --for-release --allowed"));
    let build = run_blocks(&job(&release, "build")).join("\n");
    assert!(build.contains("--example third_party_notices -- notices --for-release --allowed"));
}

#[test]
fn draft_publishes_and_digest_checks_the_metadata_assets() {
    let release = workflow("release.yaml");
    let draft = run_blocks(&job(&release, "draft")).join("\n");
    for needle in [
        "ai-guard-$RELEASE_TAG-release-metadata.tar.gz",
        "ai-guard-$RELEASE_TAG-sbom.cdx.json",
        "files=(\"${archives[@]}\" \"$sbom\" SHA256SUMS)",
        "shasum -a 256 \"${archives[@]}\" \"$sbom\" > SHA256SUMS",
        "run=%s\\nrepository=%s",
        ".metadata.component.version == $version",
    ] {
        assert!(
            draft.contains(needle),
            "the draft job's run steps lost: {needle}"
        );
    }
    let build = run_blocks(&job(&release, "build")).join("\n");
    assert!(build.contains("run=%s\\nrepository=%s") && build.contains("THIRD-PARTY-NOTICES"));
}

#[test]
fn ci_runs_the_licence_guard_and_its_unit_tests_in_the_rust_job() {
    let ci = workflow("ci.yaml");
    let rust = run_blocks(&job(&ci, "rust-check")).join("\n");
    assert!(rust.contains(
        "--example third_party_notices -- check --allowed .github/third-party-license-expressions.txt"
    ));
    assert!(rust.contains("--example third_party_notices"));
    let list = fs::read_to_string(
        Path::new(env!("CARGO_MANIFEST_DIR")).join(".github/third-party-license-expressions.txt"),
    )
    .expect("reviewed licence list is committed");
    assert!(list.lines().any(|line| line.trim() == "MIT OR Apache-2.0"));
}

fn bin_stems() -> Vec<String> {
    let mut stems: Vec<String> =
        fs::read_dir(Path::new(env!("CARGO_MANIFEST_DIR")).join("src/bin"))
            .unwrap()
            .filter_map(|entry| {
                let path = entry.unwrap().path();
                (path.extension().is_some_and(|ext| ext == "rs"))
                    .then(|| path.file_stem().unwrap().to_string_lossy().into_owned())
            })
            .collect();
    stems.sort();
    stems
}

#[test]
fn native_binary_list_comes_from_cargo_and_is_not_hard_coded() {
    let release = workflow("release.yaml");
    let verify = run_blocks(&job(&release, "verify")).join("\n");
    assert!(
        verify.contains("select(.kind | index(\"bin\"))") && verify.contains(". - ($excluded"),
        "the list must come from cargo metadata minus the declared exclusions"
    );
    // No binary named anywhere in the workflow, except the single exclusion declaration and the
    // mandatory `bin/agentsandbox`.
    let outside: String = release
        .lines()
        .filter(|line| !line.contains("EXCLUDED_BINARIES:"))
        .collect::<Vec<_>>()
        .join("\n");
    for stem in bin_stems() {
        assert!(
            !outside.contains(&stem),
            "{stem} is named in release.yaml outside EXCLUDED_BINARIES"
        );
    }
    let mut at = 0;
    while let Some(found) = outside[at..].find("bin/") {
        let name: String = outside[at + found + 4..]
            .chars()
            .take_while(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.'))
            .collect();
        assert!(
            name.is_empty() || name == "agentsandbox",
            "release.yaml names a literal bin/{name}"
        );
        at += found + 4;
    }
}

#[test]
fn the_demo_ca_generator_is_excluded_from_the_archives_and_the_exclusion_is_not_stale() {
    let release = workflow("release.yaml");
    let line = release
        .lines()
        .find(|line| line.trim_start().starts_with("EXCLUDED_BINARIES:"))
        .expect("the exclusion is declared once, in the verify job");
    let excluded: Vec<&str> = line
        .trim_start()
        .trim_start_matches("EXCLUDED_BINARIES:")
        .split_whitespace()
        .collect();
    assert_eq!(excluded, ["ai-guard-demo-pki"]);
    let stems = bin_stems();
    for name in excluded {
        assert!(
            stems.iter().any(|stem| stem == name),
            "{name} no longer exists: drop the exclusion"
        );
    }
    assert!(
        job(&release, "verify")
            .iter()
            .any(|l| l.contains("EXCLUDED_BINARIES"))
    );
}

#[test]
fn qualification_validates_the_release_tag_and_uploads_only_an_allow_list() {
    let qualification = workflow("linux-qualification.yaml");
    let runs = run_blocks(&job(&qualification, "qualify")).join("\n");
    // The tag is validated and compared with the dispatched commit before it is used.
    let validate = runs
        .find("release_tag must look like")
        .expect("tag validation");
    let download = runs.find("gh release download").expect("download");
    assert!(validate < download);
    assert!(runs.contains("release_tag_commit"));
    // Upload: only the assembled directory, never the fixture tree.
    assert!(qualification.contains("qa-evidence/${{ matrix.engine }}/upload/"));
    assert!(!qualification.contains("qa-evidence/${{ matrix.engine }}/\n"));
    assert!(runs.contains("looks like it holds a credential"));
    assert!(runs.contains("sysctl_fallback_used"));
    // The job that runs sudo and the tests never receives a write token.
    assert!(!qualification.contains("contents: write"));
}
