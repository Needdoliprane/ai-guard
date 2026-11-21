//! Third-party notices for the native release archives.
//!
//! Reads `cargo metadata --locked --offline --all-features` (so the result does not depend on the
//! default feature set or on the build host), keeps every crate reachable from `agentsandbox`
//! through normal dependencies on any platform, and prints one deterministic notice document:
//! a crate table, then each distinct licence text with the crates that carry it. Crates reached only
//! through build or dev dependencies are not linked into the binaries and are left out.
//!
//! ```text
//! cargo run --locked --offline --example third_party_notices -- notices --output FILE
//! cargo run --locked --offline --example third_party_notices -- check --allowed FILE
//! ```
//!
//! `check` fails when a linked crate has no licence expression, no licence file, or an expression
//! that is not listed in the reviewed allow-list. A `Cargo.lock` change that brings in a new
//! expression therefore needs a reviewed edit of that list. The sources must be present locally:
//! run `cargo fetch --locked` first.

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::fmt::Write as _;
use std::path::{Path, PathBuf};
use std::process::Command;

use serde_json::Value;
use sha2::{Digest, Sha256};

const ROOT_PACKAGE: &str = "agentsandbox";
const LICENSE_PREFIXES: [&str; 5] = ["license", "licence", "copying", "unlicense", "notice"];
const MAX_TEXT_BYTES: u64 = 256 * 1024;

struct Krate {
    name: String,
    version: String,
    expression: Option<String>,
    source: String,
    texts: Vec<(String, String)>,
}

fn main() {
    if let Err(message) = run() {
        eprintln!("third_party_notices: {message}");
        std::process::exit(1);
    }
}

fn run() -> Result<(), String> {
    let mut args = std::env::args().skip(1);
    let mode = args.next().ok_or("expected `notices` or `check`")?;
    let mut output = None;
    let mut allowed = None;
    while let Some(flag) = args.next() {
        let value = args.next().ok_or_else(|| format!("{flag} needs a value"))?;
        match flag.as_str() {
            "--output" => output = Some(PathBuf::from(value)),
            "--allowed" => allowed = Some(PathBuf::from(value)),
            _ => return Err(format!("unknown option {flag}")),
        }
    }
    let krates = linked_crates()?;
    match mode.as_str() {
        "notices" => {
            let output = output.ok_or("notices needs --output FILE")?;
            let problems = problems(&krates, None);
            if !problems.is_empty() {
                return Err(problems.join("\n"));
            }
            std::fs::write(&output, render(&krates))
                .map_err(|e| format!("{}: {e}", output.display()))?;
            eprintln!(
                "wrote notices for {} crates to {}",
                krates.len(),
                output.display()
            );
            Ok(())
        }
        "check" => {
            let path = allowed.ok_or("check needs --allowed FILE")?;
            let list =
                std::fs::read_to_string(&path).map_err(|e| format!("{}: {e}", path.display()))?;
            let allowed: BTreeSet<String> = list
                .lines()
                .map(str::trim)
                .filter(|line| !line.is_empty() && !line.starts_with('#'))
                .map(str::to_owned)
                .collect();
            let problems = problems(&krates, Some(&allowed));
            if problems.is_empty() {
                eprintln!(
                    "{} linked crates: licence expressions accepted",
                    krates.len()
                );
                Ok(())
            } else {
                Err(problems.join("\n"))
            }
        }
        _ => Err("expected `notices` or `check`".into()),
    }
}

fn problems(krates: &[Krate], allowed: Option<&BTreeSet<String>>) -> Vec<String> {
    let mut found = Vec::new();
    for k in krates {
        let id = format!("{} {}", k.name, k.version);
        match &k.expression {
            None if k.texts.is_empty() => {
                found.push(format!("{id}: no licence expression and no licence file"))
            }
            None => {}
            Some(expression) => {
                if let Some(allowed) = allowed {
                    if !allowed.contains(expression) {
                        found.push(format!(
                            "{id}: licence expression `{expression}` is not in the reviewed allow-list"
                        ));
                    }
                }
            }
        }
    }
    found
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
        if !dir.is_dir() {
            return Err(format!(
                "sources of {id} are not present; run `cargo fetch --locked`"
            ));
        }
        krates.push(Krate {
            name: package["name"].as_str().unwrap_or_default().to_owned(),
            version: package["version"].as_str().unwrap_or_default().to_owned(),
            expression: package["license"]
                .as_str()
                .map(|s| s.split_whitespace().collect::<Vec<_>>().join(" ")),
            source: package["source"].as_str().unwrap_or("path").to_owned(),
            texts: license_texts(dir, package["license_file"].as_str())?,
        });
    }
    krates.sort_by(|a, b| (&a.name, &a.version).cmp(&(&b.name, &b.version)));
    Ok(krates)
}

fn license_texts(dir: &Path, declared: Option<&str>) -> Result<Vec<(String, String)>, String> {
    let mut names = BTreeSet::new();
    if let Some(declared) = declared {
        names.insert(declared.to_owned());
    }
    for entry in std::fs::read_dir(dir).map_err(|e| format!("{}: {e}", dir.display()))? {
        let entry = entry.map_err(|e| e.to_string())?;
        let name = entry.file_name().to_string_lossy().into_owned();
        let lower = name.to_ascii_lowercase();
        if entry.file_type().map_err(|e| e.to_string())?.is_file()
            && LICENSE_PREFIXES
                .iter()
                .any(|prefix| lower.starts_with(prefix))
        {
            names.insert(name);
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

fn render(krates: &[Krate]) -> String {
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
            without_text.insert(format!("{} {}", k.name, k.version));
        }
        for (_, text) in &k.texts {
            let digest = hex::encode(Sha256::digest(text.as_bytes()));
            grouped
                .entry(digest)
                .or_insert_with(|| (text.clone(), BTreeSet::new()))
                .1
                .insert(format!("{} {}", k.name, k.version));
        }
    }
    if !without_text.is_empty() {
        out.push_str("\n## Crates without a licence file in their package\n\n");
        out.push_str("Only the SPDX expression declared by the crate is available for these:\n\n");
        for id in &without_text {
            let _ = writeln!(out, "- {id}");
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
