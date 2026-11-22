//! Third-party notices for the native release archives.
//!
//! Reads `cargo metadata --locked --offline --all-features` (so the result does not depend on the
//! default feature set or on the build host), keeps every crate reachable from `agentsandbox`
//! through normal dependencies on any platform, and prints one deterministic notice document:
//! a crate table, then each distinct licence text with the crates that carry it. Crates reached only
//! through build or dev dependencies are not linked into the binaries and are left out.
//!
//! ```text
//! cargo run --locked --offline --example third_party_notices -- notices --allowed LIST --output FILE
//! cargo run --locked --offline --example third_party_notices -- check --allowed LIST
//! ```
//!
//! Both modes apply the same reviewed list (`.github/third-party-license-expressions.txt`):
//!
//! - a bare line is an accepted SPDX expression;
//! - `no-expression: NAME VERSION` accepts one crate that declares no expression (its licence file
//!   must still be present, and a person has reviewed it);
//! - `no-text: NAME VERSION` accepts one crate that has an expression but ships no licence text;
//! - `supplement: NAME VERSION FILE SHA256` supplies upstream text for a crate whose published
//!   package ships none: `FILE` is read from `third-party-notices/` next to the list. It must have
//!   exactly that SHA-256 and contain non-blank UTF-8 text of at most 256 KiB. The provenance
//!   (URL, revision) of each file is recorded in the list.
//! - `licence-selection: NAME VERSION SPDX` records the selected identifier from a crate's simple
//!   `OR` expression and includes the choice in the generated notices.
//!
//! The release workflow passes `--for-release` to both modes, which forbids all `no-text:` exceptions.
//!
//! Anything else fails: an unlisted expression, an unlisted crate without expression, an unlisted
//! crate without text. A `Cargo.lock` change that brings such a crate therefore needs a reviewed edit
//! of the list. The sources must be present locally: run `cargo fetch --locked` first. Unit tests run
//! with `cargo test --example third_party_notices`.

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::fmt::Write as _;
use std::io::Read as _;
use std::path::{Path, PathBuf};
use std::process::Command;

use serde_json::Value;
use sha2::{Digest, Sha256};

const ROOT_PACKAGE: &str = "agentsandbox";
const LICENSE_PREFIXES: [&str; 6] = [
    "license",
    "licence",
    "copying",
    "copyright",
    "unlicense",
    "notice",
];
const LICENSE_DIRECTORIES: [&str; 2] = ["licenses", "licences"];
const MAX_TEXT_BYTES: u64 = 256 * 1024;

#[derive(Debug)]
struct Krate {
    name: String,
    version: String,
    expression: Option<String>,
    source: String,
    texts: Vec<(String, String)>,
}

impl Krate {
    fn id(&self) -> String {
        format!("{} {}", self.name, self.version)
    }
}

/// The reviewed list: accepted expressions and the individually reviewed exceptions.
#[derive(Debug, Default)]
struct Policy {
    expressions: BTreeSet<String>,
    no_expression: BTreeSet<String>,
    no_text: BTreeSet<String>,
    /// `NAME VERSION` to the supplied `(file, sha256)` pairs.
    supplements: BTreeMap<String, Vec<(String, String)>>,
    selected_licences: BTreeMap<String, String>,
}

impl Policy {
    fn parse(text: &str) -> Result<Self, String> {
        let mut policy = Policy::default();
        for (number, raw) in text.lines().enumerate() {
            let line = raw.split(" #").next().unwrap_or("").trim();
            if line.is_empty() || line.starts_with('#') {
                continue;
            }
            let exception = |rest: &str| -> Result<String, String> {
                let parts: Vec<_> = rest.split_whitespace().collect();
                match parts.as_slice() {
                    [name, version] => Ok(format!("{name} {version}")),
                    _ => Err(format!("line {}: expected `NAME VERSION`", number + 1)),
                }
            };
            if let Some(rest) = line.strip_prefix("supplement:") {
                let parts: Vec<_> = rest.split_whitespace().collect();
                let [name, version, file, sha] = parts.as_slice() else {
                    return Err(format!(
                        "line {}: expected `NAME VERSION FILE SHA256`",
                        number + 1
                    ));
                };
                if sha.len() != 64
                    || !sha
                        .bytes()
                        .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
                    || file.contains(['/', '\\'])
                    || file.starts_with('.')
                {
                    return Err(format!("line {}: invalid file name or SHA-256", number + 1));
                }
                policy
                    .supplements
                    .entry(format!("{name} {version}"))
                    .or_default()
                    .push(((*file).to_owned(), (*sha).to_owned()));
            } else if let Some(rest) = line.strip_prefix("licence-selection:") {
                let parts: Vec<_> = rest.split_whitespace().collect();
                let [name, version, licence] = parts.as_slice() else {
                    return Err(format!("line {}: expected `NAME VERSION SPDX`", number + 1));
                };
                let id = format!("{name} {version}");
                if policy
                    .selected_licences
                    .insert(id.clone(), (*licence).to_owned())
                    .is_some()
                {
                    return Err(format!(
                        "line {}: duplicate licence selection for {id}",
                        number + 1
                    ));
                }
            } else if let Some(rest) = line.strip_prefix("no-expression:") {
                policy.no_expression.insert(exception(rest)?);
            } else if let Some(rest) = line.strip_prefix("no-text:") {
                policy.no_text.insert(exception(rest)?);
            } else {
                policy.expressions.insert(line.to_owned());
            }
        }
        Ok(policy)
    }
}

fn main() {
    if let Err(message) = run(std::env::args().skip(1)) {
        eprintln!("third_party_notices: {message}");
        std::process::exit(1);
    }
}

fn run(mut args: impl Iterator<Item = String>) -> Result<(), String> {
    let mode = args.next().ok_or("expected `notices` or `check`")?;
    let mut output = None;
    let mut allowed = None;
    let mut for_release = false;
    while let Some(flag) = args.next() {
        if flag == "--for-release" {
            for_release = true;
            continue;
        }
        let value = args.next().ok_or_else(|| format!("{flag} needs a value"))?;
        match flag.as_str() {
            "--output" => output = Some(PathBuf::from(value)),
            "--allowed" => allowed = Some(PathBuf::from(value)),
            _ => return Err(format!("unknown option {flag}")),
        }
    }
    let path = allowed.ok_or("--allowed LIST is required")?;
    let list = std::fs::read_to_string(&path).map_err(|e| format!("{}: {e}", path.display()))?;
    let policy = Policy::parse(&list)?;
    if for_release && !policy.no_text.is_empty() {
        return Err(
            "release policy forbids `no-text:` exceptions; supply upstream licence texts with `supplement:`"
                .into(),
        );
    }
    let mut krates = linked_crates()?;
    let directory = path
        .parent()
        .unwrap_or_else(|| Path::new("."))
        .join("third-party-notices");
    apply_supplements(&mut krates, &policy, &|file| {
        let mut bytes = Vec::new();
        std::fs::File::open(directory.join(file))
            .and_then(|source| source.take(MAX_TEXT_BYTES + 1).read_to_end(&mut bytes))
            .map_err(|e| format!("{file}: {e}"))?;
        Ok(bytes)
    })?;
    let problems = problems(&krates, &policy);
    if !problems.is_empty() {
        return Err(problems.join("\n"));
    }
    match mode.as_str() {
        "notices" => {
            let output = output.ok_or("notices needs --output FILE")?;
            std::fs::write(&output, render(&krates, &policy))
                .map_err(|e| format!("{}: {e}", output.display()))?;
            eprintln!(
                "wrote notices for {} crates to {}",
                krates.len(),
                output.display()
            );
            Ok(())
        }
        "check" => {
            eprintln!(
                "{} linked crates: licence expressions and texts accepted",
                krates.len()
            );
            Ok(())
        }
        _ => Err("expected `notices` or `check`".into()),
    }
}

/// Adds the supplied upstream texts. Each supplement must name a linked crate that ships no text
/// of its own (otherwise it is stale) and its bytes must have the recorded SHA-256.
fn apply_supplements(
    krates: &mut [Krate],
    policy: &Policy,
    read: &dyn Fn(&str) -> Result<Vec<u8>, String>,
) -> Result<(), String> {
    for (id, files) in &policy.supplements {
        let krate = krates
            .iter_mut()
            .find(|k| k.id() == *id)
            .ok_or_else(|| format!("{id}: supplement for a crate that is not linked"))?;
        if !krate.texts.is_empty() {
            return Err(format!(
                "{id}: ships its own licence text; drop the supplement"
            ));
        }
        for (file, sha) in files {
            let bytes = read(file)?;
            if bytes.len() as u64 > MAX_TEXT_BYTES {
                return Err(format!("{id}: {file} exceeds {MAX_TEXT_BYTES} bytes"));
            }
            let actual = hex::encode(Sha256::digest(&bytes));
            if actual != *sha {
                return Err(format!(
                    "{id}: {file} has SHA-256 {actual}, the list records {sha}"
                ));
            }
            let text = std::str::from_utf8(&bytes)
                .map_err(|_| format!("{id}: {file} is not valid UTF-8"))?;
            if text.trim().is_empty() {
                return Err(format!("{id}: {file} contains no licence text"));
            }
            let text = text.replace("\r\n", "\n");
            krate.texts.push((file.clone(), text.trim_end().to_owned()));
        }
    }
    Ok(())
}

fn problems(krates: &[Krate], policy: &Policy) -> Vec<String> {
    let mut found = Vec::new();
    for (id, selected) in &policy.selected_licences {
        let expression = krates
            .iter()
            .find(|krate| krate.id() == *id)
            .and_then(|krate| krate.expression.as_deref());
        let valid = expression.is_some_and(|expression| {
            !expression.contains(['(', ')'])
                && expression.contains(" OR ")
                && expression
                    .split(" OR ")
                    .any(|alternative| alternative == selected)
        });
        if !valid {
            found.push(format!(
                "{id}: `{selected}` is not a declared licence alternative"
            ));
        }
    }
    for k in krates {
        let id = k.id();
        match &k.expression {
            None => {
                if !policy.no_expression.contains(&id) {
                    found.push(format!(
                        "{id}: declares no licence expression and is not in the reviewed `no-expression:` list"
                    ));
                }
                if k.texts.is_empty() {
                    found.push(format!("{id}: no licence expression and no licence text"));
                }
            }
            Some(expression) => {
                if !policy.expressions.contains(expression) {
                    found.push(format!(
                        "{id}: licence expression `{expression}` is not in the reviewed list"
                    ));
                }
                if k.texts.is_empty() && !policy.no_text.contains(&id) {
                    found.push(format!(
                        "{id}: ships no licence text and is not in the reviewed `no-text:` list"
                    ));
                }
            }
        }
    }
    found
}

/// Crates reachable from the root through normal dependencies, with their licence information.
/// `texts` reads the licence files of a package directory; it is a parameter so that tests can
/// feed a fixture instead of a registry checkout.
fn crates_from(
    meta: &Value,
    texts: &dyn Fn(&Path, Option<&str>) -> Result<Vec<(String, String)>, String>,
) -> Result<Vec<Krate>, String> {
    let packages = meta["packages"]
        .as_array()
        .ok_or("metadata has no packages")?;
    let by_id: HashMap<&str, &Value> = packages
        .iter()
        .filter_map(|p| p["id"].as_str().map(|id| (id, p)))
        .collect();
    let root = packages
        .iter()
        .find(|p| p["name"] == ROOT_PACKAGE)
        .and_then(|p| p["id"].as_str())
        .ok_or("root package not found")?;
    let nodes: HashMap<&str, &Value> = meta["resolve"]["nodes"]
        .as_array()
        .ok_or("metadata has no resolve graph")?
        .iter()
        .filter_map(|n| n["id"].as_str().map(|id| (id, n)))
        .collect();

    let mut seen = BTreeSet::new();
    let mut stack = vec![root];
    while let Some(id) = stack.pop() {
        if !seen.insert(id) {
            continue;
        }
        for dep in nodes
            .get(id)
            .and_then(|n| n["deps"].as_array())
            .into_iter()
            .flatten()
        {
            // `kind: null` is a normal dependency; "dev" and "build" edges are not linked.
            let normal = dep["dep_kinds"]
                .as_array()
                .is_some_and(|kinds| kinds.iter().any(|kind| kind["kind"].is_null()));
            if let (true, Some(next)) = (normal, dep["pkg"].as_str()) {
                stack.push(next);
            }
        }
    }

    let mut krates = Vec::new();
    for id in seen.into_iter().filter(|id| *id != root) {
        let package = by_id
            .get(id)
            .ok_or_else(|| format!("{id} missing from packages"))?;
        let manifest = package["manifest_path"]
            .as_str()
            .ok_or("package without manifest_path")?;
        let dir = Path::new(manifest)
            .parent()
            .ok_or("manifest without parent")?;
        krates.push(Krate {
            name: package["name"].as_str().unwrap_or_default().to_owned(),
            version: package["version"].as_str().unwrap_or_default().to_owned(),
            expression: package["license"]
                .as_str()
                .map(|s| s.split_whitespace().collect::<Vec<_>>().join(" ")),
            source: package["source"].as_str().unwrap_or("path").to_owned(),
            texts: texts(dir, package["license_file"].as_str())?,
        });
    }
    krates.sort_by(|a, b| (&a.name, &a.version).cmp(&(&b.name, &b.version)));
    Ok(krates)
}

fn linked_crates() -> Result<Vec<Krate>, String> {
    let cargo = std::env::var("CARGO").unwrap_or_else(|_| "cargo".into());
    let output = Command::new(cargo)
        .args([
            "metadata",
            "--locked",
            "--offline",
            "--all-features",
            "--format-version",
            "1",
        ])
        .output()
        .map_err(|e| format!("cannot run cargo metadata: {e}"))?;
    if !output.status.success() {
        return Err(format!(
            "cargo metadata failed (run `cargo fetch --locked` first): {}",
            String::from_utf8_lossy(&output.stderr).trim()
        ));
    }
    let meta: Value =
        serde_json::from_slice(&output.stdout).map_err(|e| format!("metadata JSON: {e}"))?;
    crates_from(&meta, &|dir, declared| {
        if !dir.is_dir() {
            return Err(format!(
                "sources in {} are not present; run `cargo fetch --locked`",
                dir.display()
            ));
        }
        license_texts(dir, declared)
    })
}

fn is_license_name(name: &str) -> bool {
    let lower = name.to_ascii_lowercase();
    LICENSE_PREFIXES
        .iter()
        .any(|prefix| lower.starts_with(prefix))
}

fn license_texts(dir: &Path, declared: Option<&str>) -> Result<Vec<(String, String)>, String> {
    let mut names = BTreeSet::new();
    if let Some(declared) = declared {
        names.insert(declared.to_owned());
    }
    for entry in std::fs::read_dir(dir).map_err(|e| format!("{}: {e}", dir.display()))? {
        let entry = entry.map_err(|e| e.to_string())?;
        let name = entry.file_name().to_string_lossy().into_owned();
        let kind = entry.file_type().map_err(|e| e.to_string())?;
        if kind.is_file() && is_license_name(&name) {
            names.insert(name);
        } else if kind.is_dir() && LICENSE_DIRECTORIES.contains(&name.to_ascii_lowercase().as_str())
        {
            // REUSE-style `LICENSES/<id>.txt`: one level only.
            for inner in std::fs::read_dir(entry.path()).map_err(|e| e.to_string())? {
                let inner = inner.map_err(|e| e.to_string())?;
                if inner.file_type().map_err(|e| e.to_string())?.is_file() {
                    names.insert(format!("{name}/{}", inner.file_name().to_string_lossy()));
                }
            }
        }
    }
    let mut texts = Vec::new();
    for name in names {
        let path = dir.join(&name);
        let Ok(meta) = std::fs::metadata(&path) else {
            continue;
        };
        if !meta.is_file() || meta.len() > MAX_TEXT_BYTES {
            continue;
        }
        let bytes = std::fs::read(&path).map_err(|e| format!("{}: {e}", path.display()))?;
        texts.push((
            name,
            String::from_utf8_lossy(&bytes)
                .replace("\r\n", "\n")
                .trim_end()
                .to_owned(),
        ));
    }
    Ok(texts)
}

fn render(krates: &[Krate], policy: &Policy) -> String {
    let mut out = String::new();
    out.push_str("# Third-party notices\n\n");
    out.push_str("ai-guard native binaries statically link the Rust crates listed below. ");
    out.push_str("The list is every crate reachable from `agentsandbox` through normal dependencies on any platform ");
    out.push_str(
        "and with all features, taken from `Cargo.lock`; it can be a superset of one build. ",
    );
    out.push_str("The Rust standard library and the system C library are not covered here.\n\n");
    let _ = writeln!(out, "Crates: {}\n", krates.len());
    out.push_str("| Crate | Version | Licence expression | Source |\n|---|---|---|---|\n");
    for k in krates {
        let _ = writeln!(
            out,
            "| {} | {} | {} | {} |",
            k.name,
            k.version,
            k.expression.as_deref().unwrap_or("see licence file"),
            k.source
        );
    }
    let mut grouped: BTreeMap<String, (String, BTreeSet<String>)> = BTreeMap::new();
    let mut without_text = BTreeSet::new();
    for k in krates {
        if k.texts.is_empty() {
            without_text.insert(k.id());
        }
        for (_, text) in &k.texts {
            let digest = hex::encode(Sha256::digest(text.as_bytes()));
            grouped
                .entry(digest)
                .or_insert_with(|| (text.clone(), BTreeSet::new()))
                .1
                .insert(k.id());
        }
    }
    if !without_text.is_empty() {
        out.push_str("\n## Crates without a licence file in their package\n\n");
        out.push_str("Only the SPDX expression declared by the crate is available for these (reviewed individually):\n\n");
        for id in &without_text {
            let _ = writeln!(out, "- {id}");
        }
    }
    if !policy.selected_licences.is_empty() {
        out.push_str("\n## Licences selected for distribution\n\n");
        for (id, licence) in &policy.selected_licences {
            let _ = writeln!(out, "- `{id}` uses `{licence}`.");
        }
    }
    out.push_str("\n## Licence texts\n");
    for (digest, (text, owners)) in &grouped {
        let _ = writeln!(out, "\n### Text {}\n", &digest[..12]);
        let _ = writeln!(
            out,
            "Carried by: {}\n",
            owners.iter().cloned().collect::<Vec<_>>().join(", ")
        );
        out.push_str("~~~~text\n");
        out.push_str(text);
        out.push_str("\n~~~~\n");
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn package(name: &str, license: Option<&str>) -> Value {
        json!({
            "id": name, "name": name, "version": "1.0.0", "license": license,
            "license_file": null, "source": "registry+fixture",
            "manifest_path": format!("/fixture/{name}/Cargo.toml"),
        })
    }

    /// root -> a (normal) -> d (normal); root -> dev_only (dev); root -> build_only (build).
    fn metadata() -> Value {
        let edge = |pkg: &str, kind: Value| json!({"pkg": pkg, "dep_kinds": [{"kind": kind, "target": null}]});
        json!({
            "packages": [
                package(ROOT_PACKAGE, Some("Apache-2.0")),
                package("a", Some("MIT")),
                package("d", Some("MIT OR Apache-2.0")),
                package("dev_only", Some("GPL-3.0")),
                package("build_only", Some("GPL-3.0")),
            ],
            "resolve": {"nodes": [
                {"id": ROOT_PACKAGE, "deps": [
                    edge("a", Value::Null), edge("dev_only", json!("dev")), edge("build_only", json!("build"))]},
                {"id": "a", "deps": [edge("d", Value::Null)]},
                {"id": "d", "deps": []},
                {"id": "dev_only", "deps": []},
                {"id": "build_only", "deps": []},
            ]},
        })
    }

    fn with_texts(files: &[(&str, &str)]) -> Vec<Krate> {
        let files: HashMap<String, String> = files
            .iter()
            .map(|(name, text)| (format!("/fixture/{name}"), (*text).to_owned()))
            .collect();
        crates_from(&metadata(), &|dir, _| {
            Ok(files
                .get(dir.to_str().unwrap())
                .map(|text| vec![("LICENSE".to_owned(), text.clone())])
                .unwrap_or_default())
        })
        .unwrap()
    }

    fn policy(text: &str) -> Policy {
        Policy::parse(text).unwrap()
    }

    #[test]
    fn release_commands_refuse_a_missing_text_exception_before_generating_notices() {
        let dir = tempfile::tempdir().unwrap();
        let policy = dir.path().join("allowed.txt");
        let output = dir.path().join("THIRD-PARTY-NOTICES");
        std::fs::write(&policy, "MIT\n  no-text: fixture 1.0.0 # reviewed\n").unwrap();
        for mode in ["check", "notices"] {
            let args = [
                mode,
                "--for-release",
                "--allowed",
                policy.to_str().unwrap(),
                "--output",
                output.to_str().unwrap(),
            ];
            assert_eq!(
                run(args.into_iter().map(str::to_owned)).unwrap_err(),
                "release policy forbids `no-text:` exceptions; supply upstream licence texts with `supplement:`"
            );
            assert!(!output.exists());
        }
    }

    #[test]
    fn only_normal_edges_are_linked() {
        let names: Vec<_> = with_texts(&[]).into_iter().map(|k| k.name).collect();
        assert_eq!(names, ["a", "d"]);
    }

    #[test]
    fn accepted_when_listed_and_texts_present() {
        let krates = with_texts(&[("a", "text a"), ("d", "text d")]);
        assert!(problems(&krates, &policy("MIT\nMIT OR Apache-2.0\n")).is_empty());
    }

    #[test]
    fn unlisted_expression_is_refused() {
        let krates = with_texts(&[("a", "text a"), ("d", "text d")]);
        let found = problems(&krates, &policy("MIT\n"));
        assert_eq!(found.len(), 1);
        assert!(found[0].contains("d 1.0.0") && found[0].contains("MIT OR Apache-2.0"));
    }

    #[test]
    fn missing_text_is_refused_unless_reviewed() {
        let krates = with_texts(&[("a", "text a")]);
        let list = "MIT\nMIT OR Apache-2.0\n";
        let found = problems(&krates, &policy(list));
        assert!(
            found
                .iter()
                .any(|m| m.contains("d 1.0.0") && m.contains("no licence text"))
        );
        let reviewed = format!("{list}no-text: d 1.0.0  # reviewed: expression only\n");
        assert!(problems(&krates, &policy(&reviewed)).is_empty());
    }

    #[test]
    fn no_expression_is_refused_even_with_a_licence_file_unless_listed() {
        let mut metadata = metadata();
        metadata["packages"][1]["license"] = Value::Null;
        let krates = crates_from(&metadata, &|_, _| {
            Ok(vec![(
                "LICENSE".to_owned(),
                "GNU GENERAL PUBLIC LICENSE".to_owned(),
            )])
        })
        .unwrap();
        let list = "MIT OR Apache-2.0\n";
        let found = problems(&krates, &policy(list));
        assert!(
            found
                .iter()
                .any(|m| m.contains("a 1.0.0") && m.contains("no-expression"))
        );
        let reviewed = format!("{list}no-expression: a 1.0.0\n");
        assert!(problems(&krates, &policy(&reviewed)).is_empty());
    }

    #[test]
    fn no_expression_and_no_text_is_always_refused() {
        let mut metadata = metadata();
        metadata["packages"][1]["license"] = Value::Null;
        let krates = crates_from(&metadata, &|_, _| Ok(Vec::new())).unwrap();
        let list = "MIT OR Apache-2.0\nno-expression: a 1.0.0\nno-text: a 1.0.0\n";
        let found = problems(&krates, &policy(list));
        assert!(
            found
                .iter()
                .any(|m| m.contains("no licence expression and no licence text"))
        );
    }

    fn hash(text: &str) -> String {
        hex::encode(Sha256::digest(text.as_bytes()))
    }

    #[test]
    fn a_supplement_with_the_recorded_hash_clears_the_missing_text() {
        let mut krates = with_texts(&[("a", "text a")]);
        let list = format!(
            "MIT\nMIT OR Apache-2.0\nsupplement: d 1.0.0 d.txt {}\n",
            hash("upstream")
        );
        let policy = policy(&list);
        assert!(!problems(&krates, &policy).is_empty());
        apply_supplements(&mut krates, &policy, &|_| Ok(b"upstream\n".to_vec())).unwrap_err();
        apply_supplements(&mut krates, &policy, &|_| Ok(b"upstream".to_vec())).unwrap();
        assert!(problems(&krates, &policy).is_empty());
    }

    #[test]
    fn a_valid_supplement_digest_does_not_replace_text_validation() {
        let oversized = vec![b'a'; MAX_TEXT_BYTES as usize + 1];
        let cases: &[(&[u8], &str)] = &[
            (b"", "contains no licence text"),
            (b" \t\r\n", "contains no licence text"),
            (b"license\xff\n", "is not valid UTF-8"),
            (&oversized, "exceeds 262144 bytes"),
        ];
        for (bytes, reason) in cases {
            let mut krates = with_texts(&[("a", "text a")]);
            let digest = hex::encode(Sha256::digest(bytes));
            let list = format!("MIT\nMIT OR Apache-2.0\nsupplement: d 1.0.0 d.txt {digest}\n");
            let error = apply_supplements(&mut krates, &policy(&list), &|_| Ok(bytes.to_vec()))
                .unwrap_err();
            assert_eq!(error, format!("d 1.0.0: d.txt {reason}"));
        }
    }

    #[test]
    fn a_supplement_is_refused_when_stale_unlinked_or_malformed() {
        let hex = hash("x");
        let mut krates = with_texts(&[("a", "text a"), ("d", "text d")]);
        let stale = policy(&format!("supplement: d 1.0.0 d.txt {hex}\n"));
        assert!(apply_supplements(&mut krates, &stale, &|_| Ok(b"x".to_vec())).is_err());
        let unlinked = policy(&format!("supplement: nowhere 1.0.0 n.txt {hex}\n"));
        assert!(apply_supplements(&mut krates, &unlinked, &|_| Ok(b"x".to_vec())).is_err());
        assert!(Policy::parse("supplement: d 1.0.0 d.txt abc\n").is_err());
        assert!(Policy::parse(&format!("supplement: d 1.0.0 ../d.txt {hex}\n")).is_err());
    }

    #[test]
    fn notices_state_the_selected_alternative_and_refuse_an_undeclared_choice() {
        let krates = with_texts(&[("a", "text a"), ("d", "text d")]);
        let chosen = policy("MIT\nMIT OR Apache-2.0\nlicence-selection: d 1.0.0 MIT\n");
        assert!(problems(&krates, &chosen).is_empty());
        assert!(render(&krates, &chosen).contains("- `d 1.0.0` uses `MIT`."));

        let undeclared = policy("MIT\nMIT OR Apache-2.0\nlicence-selection: d 1.0.0 Zlib\n");
        assert_eq!(
            problems(&krates, &undeclared),
            ["d 1.0.0: `Zlib` is not a declared licence alternative"]
        );
    }

    #[test]
    fn exception_lines_need_a_name_and_a_version() {
        assert!(Policy::parse("no-text: only-a-name\n").is_err());
    }

    #[test]
    fn discovery_reads_copyright_files_and_reuse_directories() {
        let dir = std::env::temp_dir().join(format!("third-party-notices-{}", std::process::id()));
        std::fs::create_dir_all(dir.join("LICENSES")).unwrap();
        std::fs::write(dir.join("COPYRIGHT"), "Copyright holders").unwrap();
        std::fs::write(dir.join("LICENSES/MIT.txt"), "MIT text").unwrap();
        std::fs::write(dir.join("README.md"), "not a licence").unwrap();
        let names: Vec<_> = license_texts(&dir, None)
            .unwrap()
            .into_iter()
            .map(|t| t.0)
            .collect();
        std::fs::remove_dir_all(&dir).unwrap();
        assert_eq!(names, ["COPYRIGHT", "LICENSES/MIT.txt"]);
    }
}
