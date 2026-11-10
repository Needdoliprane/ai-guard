//! Bounded reads of handed-over public files and exclusive writes of outputs.
//!
//! Inputs are read through the trusted projected-file policy of `control_fs`:
//! a regular single-link file owned by root or the current account, not
//! writable by group or others, on a supported local filesystem, under
//! ancestors with the same ownership rule. Outputs are created exclusively
//! with mode 0600 and are never overwritten, renamed or removed.

use super::report::{Code, Failure};
use crate::control_fs::{absolute_path, read_trusted_projected_file};
use std::{
    fs::OpenOptions,
    io::Write,
    os::unix::fs::{MetadataExt, OpenOptionsExt},
    path::{Component, Path, PathBuf},
};

pub(crate) fn absolute(path: &Path) -> Result<PathBuf, Failure> {
    absolute_path(path).map_err(|error| {
        Failure::new(
            Code::InvalidArgument,
            "a path argument is not usable without links or parent traversal",
        )
        .with("path", path.display().to_string())
        .cause(&error)
    })
}

pub(crate) fn io_failure(message: &str, error: &anyhow::Error) -> Failure {
    Failure::new(Code::IoFailure, message).cause(error)
}

pub(crate) fn already_exists(error: &anyhow::Error) -> bool {
    error.chain().any(|cause| {
        cause
            .downcast_ref::<std::io::Error>()
            .is_some_and(|io| io.kind() == std::io::ErrorKind::AlreadyExists)
    })
}

/// Bounded read of a public input. `input` names the argument in the error.
pub(crate) fn read_input(
    path: &Path,
    max_bytes: u64,
    code: Code,
    input: &str,
) -> Result<Vec<u8>, Failure> {
    let path = absolute(path)?;
    read_trusted_projected_file(&path, max_bytes).map_err(|error| {
        Failure::new(
            code,
            "the input file cannot be read under the trusted local file policy",
        )
        .with("input", input)
        .with("path", path.display().to_string())
        .cause(&error)
    })
}

#[cfg(any(target_os = "linux", target_os = "macos"))]
fn check_directory(directory: &std::fs::File) -> anyhow::Result<()> {
    crate::control_fs::check_export_directory(directory)
}

#[cfg(not(any(target_os = "linux", target_os = "macos")))]
fn check_directory(_directory: &std::fs::File) -> anyhow::Result<()> {
    anyhow::bail!("output directories require a supported local Linux or macOS filesystem")
}

/// Refuse an output location that the trusted reader would refuse afterwards.
fn check_output_parent(parent: &Path) -> anyhow::Result<()> {
    let mut prefix = PathBuf::new();
    for component in parent.components() {
        match component {
            Component::RootDir | Component::Normal(_) => prefix.push(component.as_os_str()),
            _ => anyhow::bail!("output path is not canonical"),
        }
        let directory = OpenOptions::new()
            .read(true)
            .custom_flags(libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_CLOEXEC)
            .open(&prefix)?;
        check_directory(&directory)?;
    }
    Ok(())
}

fn output_exists(path: &Path) -> Failure {
    Failure::new(Code::OutputExists, "the output path already exists")
        .with("path", path.display().to_string())
}

/// Refuse an output path that already names anything, or whose directory the
/// trusted reader would refuse afterwards, before any work whose result would
/// then have nowhere to go.
pub(crate) fn require_new_output(path: &Path) -> Result<PathBuf, Failure> {
    let path = absolute(path)?;
    let shown = path.display().to_string();
    match std::fs::symlink_metadata(&path) {
        Ok(_) => return Err(output_exists(&path)),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => {
            return Err(Failure::new(
                Code::OutputUnavailable,
                "the output path cannot be inspected",
            )
            .with("path", shown)
            .cause(&error.into()));
        }
    }
    let parent = path.parent().ok_or_else(|| {
        Failure::new(
            Code::InvalidArgument,
            "the output path has no parent directory",
        )
        .with("path", shown.clone())
    })?;
    check_output_parent(parent).map_err(|error| {
        Failure::new(
            Code::OutputUnavailable,
            "the output directory does not satisfy the trusted local file policy",
        )
        .with("path", shown.clone())
        .cause(&error)
    })?;
    Ok(path)
}

/// Create `path` exclusively with exactly `bytes`, make it durable and prove
/// that the trusted reader returns the same bytes. A failure after creation
/// leaves the partial file in place for diagnosis.
pub(crate) fn write_new(path: &Path, bytes: &[u8], max_bytes: u64) -> Result<PathBuf, Failure> {
    let path = require_new_output(path)?;
    let shown = path.display().to_string();
    let exists = || output_exists(&path);
    let parent = path.parent().ok_or_else(|| {
        Failure::new(
            Code::InvalidArgument,
            "the output path has no parent directory",
        )
        .with("path", shown.clone())
    })?;
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC)
        .open(&path)
        .map_err(|error| {
            if error.kind() == std::io::ErrorKind::AlreadyExists {
                exists()
            } else {
                io_failure("the output file could not be created", &error.into())
                    .with("path", shown.clone())
            }
        })?;
    let durable = (|| -> std::io::Result<()> {
        file.write_all(bytes)?;
        file.sync_all()?;
        std::fs::File::open(parent)?.sync_all()
    })();
    durable.map_err(|error| {
        io_failure(
            "the output file could not be written durably",
            &error.into(),
        )
        .with("path", shown.clone())
    })?;
    drop(file);
    let written = read_trusted_projected_file(&path, max_bytes).map_err(|error| {
        io_failure("the output file could not be read back", &error).with("path", shown.clone())
    })?;
    if written != bytes {
        return Err(Failure::new(
            Code::IoFailure,
            "the output file read back differs from what was written",
        )
        .with("path", shown));
    }
    Ok(path)
}

/// Filesystem device of an existing path, when it can be observed.
pub(crate) fn device_of(path: &Path) -> Option<u64> {
    std::fs::symlink_metadata(path)
        .ok()
        .map(|facts| facts.dev())
}
