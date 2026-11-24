use std::{
    collections::{BTreeMap, BTreeSet},
    fs::File,
    io::Read,
    path::Path,
};

use anyhow::{Context, Result, ensure};
use flate2::read::MultiGzDecoder;

#[derive(Clone, Copy)]
enum Member {
    Directory,
    File { size: u64, mode: u32 },
}

pub(super) fn verify(archive: &Path, root: &str, target: &str, native_bins: &str) -> Result<()> {
    verify_members(archive, root, target, native_bins)
        .with_context(|| format!("invalid release archive {}", archive.display()))
}

fn verify_members(archive: &Path, root: &str, target: &str, native_bins: &str) -> Result<()> {
    ensure!(valid_component(root), "invalid release root {root:?}");
    let mut required = vec!["BUILD-INFO", "LICENSE"];
    let mut binaries = BTreeSet::new();
    match target {
        "release-metadata" => required.extend([
            "manifest/release-manifest.json",
            "manifest/sbom.cdx.json",
            "manifest/SHA256SUMS",
        ]),
        "frontend" => required.extend(["index.html", "licenses/vite-LICENSE.md"]),
        "x86_64-unknown-linux-gnu" | "aarch64-apple-darwin" => {
            required.push("THIRD-PARTY-NOTICES");
            for program in native_bins.split_ascii_whitespace() {
                ensure!(
                    valid_component(program) && binaries.insert(format!("bin/{program}")),
                    "invalid or duplicate native binary name {program:?}"
                );
            }
            ensure!(
                binaries.contains("bin/agentsandbox"),
                "missing verified agentsandbox binary"
            );
        }
        _ => anyhow::bail!("unknown release target {target:?}"),
    }

    let mut archive = tar::Archive::new(MultiGzDecoder::new(File::open(archive)?));
    let mut members = BTreeMap::new();
    let mut root_seen = false;
    let prefix = format!("{root}/");
    for entry in archive.entries()? {
        let entry = entry?;
        let path = entry.path_bytes();
        let path = std::str::from_utf8(&path).context("release member path must be UTF-8")?;
        let kind = entry.header().entry_type();
        let member = if kind.is_dir() {
            Member::Directory
        } else if kind.is_file() {
            Member::File {
                size: entry.size(),
                mode: entry.header().mode()?,
            }
        } else {
            anyhow::bail!("unsupported member type for {path:?}");
        };
        let path = if kind.is_dir() {
            path.strip_suffix('/').unwrap_or(path)
        } else {
            path
        };
        if path == root {
            ensure!(kind.is_dir(), "root must be a directory");
            ensure!(!root_seen, "duplicate root directory");
            root_seen = true;
            continue;
        }
        let relative = path
            .strip_prefix(&prefix)
            .filter(|p| {
                p.split('/').all(|part| {
                    !matches!(part, "" | "." | "..") && !part.chars().any(char::is_control)
                })
            })
            .with_context(|| format!("invalid member path {path:?}"))?;
        ensure!(
            members.insert(relative.to_owned(), member).is_none(),
            "duplicate member {relative:?}"
        );
    }

    // tar stops at the first end block. Consume padding and verify the gzip trailer.
    let mut decoder = archive.into_inner();
    let mut padding = [0; 8192];
    loop {
        let count = decoder.read(&mut padding)?;
        if count == 0 {
            break;
        }
        ensure!(
            padding[..count].iter().all(|&byte| byte == 0),
            "unexpected data after tar end marker"
        );
    }

    ensure!(root_seen, "missing root directory");
    for (path, member) in &members {
        if let Some((parent, _)) = path.rsplit_once('/') {
            ensure!(
                matches!(members.get(parent), Some(Member::Directory)),
                "parent is not a declared directory for {path:?}: {parent:?}"
            );
        }
        if !binaries.is_empty()
            && ((path == "bin" && !matches!(member, Member::Directory))
                || (path.starts_with("bin/") && !binaries.contains(path)))
        {
            anyhow::bail!("Unexpected native bin member {path}");
        }
    }
    for path in required {
        ensure!(
            matches!(members.get(path), Some(Member::File { size, .. }) if *size > 0),
            "expected one nonempty regular member: {path}"
        );
    }
    for path in binaries {
        ensure!(
            matches!(members.get(&path), Some(Member::File { size, mode }) if *size > 0 && mode & 0o100 != 0),
            "expected one nonempty owner-executable regular member: {path}"
        );
    }
    if target == "frontend" {
        ensure!(
            members.iter().any(|(path, member)| {
                path.strip_prefix("assets/")
                    .is_some_and(|asset| asset.ends_with(".js") && asset.len() > 3)
                    && matches!(member, Member::File { size, .. } if *size > 0)
            }),
            "expected a nonempty regular JavaScript asset under assets/"
        );
    }
    if target == "release-metadata" {
        ensure!(
            !members.contains_key("index.html") && !members.contains_key("THIRD-PARTY-NOTICES"),
            "unexpected frontend or notices member in the metadata archive"
        );
    }
    Ok(())
}

fn valid_component(value: &str) -> bool {
    !matches!(value, "" | "." | "..")
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || b"_.-".contains(&byte))
}
