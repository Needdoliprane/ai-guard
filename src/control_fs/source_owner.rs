//! Physical source identities and observed same-mount bindings.
//!
//! These checks retain the existing trusted-UID/root boundary. They observe
//! identities and mount domains; they do not acquire ownership locks, fence
//! SQLite writers or qualify a filesystem/kernel deployment.

use super::{Directory, Identity, PrivateEntryIdentity, check_directory, child_name};
use anyhow::{Context, Result, ensure};
use std::{
    ffi::{CStr, OsStr},
    fmt,
    fs::File,
    os::fd::{AsRawFd, RawFd},
};

/// Internal phase only. The context carries no private data and keeps the
/// original error chain available for structured descriptor-shortage checks.
#[derive(Debug)]
pub(crate) struct InitialParentObservationFailure;

impl fmt::Display for InitialParentObservationFailure {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("private source initial parent observation failed")
    }
}

impl std::error::Error for InitialParentObservationFailure {}

impl PrivateEntryIdentity {
    /// Physical identity of the entry inspected by the existing private guard.
    pub(crate) fn physical_identity(self) -> (u64, u64) {
        (self.device, self.inode)
    }
}

impl Directory {
    /// Revalidate the path and inspect the retained descriptor, never a new
    /// path-based file descriptor or an unverified serialization of its name.
    pub(crate) fn physical_identity(&self) -> Result<(u64, u64)> {
        self.revalidate()?;
        let facts = check_directory(&self.0.file, true)?;
        let identity = Identity::of(&facts);
        ensure!(
            self.0.chain.last() == Some(&identity),
            "retained source directory identity changed"
        );
        self.revalidate()?;
        Ok((identity.device, identity.inode))
    }

    /// Require an existing private regular entry in the parent's actual mount
    /// domain. A file-only mount must not choose a different owner-lock parent.
    /// No descriptor to the entry (including SQLite main/SHM) is opened here.
    /// Missing, unsupported and incomplete native observations all refuse.
    pub(crate) fn private_entry_same_mount(&self, name: &OsStr) -> Result<PrivateEntryIdentity> {
        self.private_entry_same_mount_with_parent(name)
            .map(|(_, entry)| entry)
    }

    /// The parent and entry are observations of this same compound inspection,
    /// allowing the caller to compare both with its exact storage binding.
    pub(crate) fn private_entry_same_mount_with_parent(
        &self,
        name: &OsStr,
    ) -> Result<((u64, u64), PrivateEntryIdentity)> {
        let encoded = child_name(name)?;
        let parent = self
            .physical_identity()
            .context(InitialParentObservationFailure)?;
        // physical_identity ends with a full path revalidation. Inspect the
        // entry immediately, without repeating that same walk between helpers.
        let entry = self
            .private_entry_after_parent_revalidation(name)?
            .context("private source entry is absent")?;
        let before = observe_same_mount(&self.0.file, &encoded, parent, entry.physical_identity())?;

        ensure!(
            self.physical_identity()? == parent
                && self.private_entry_after_parent_revalidation(name)? == Some(entry),
            "private source identity changed during mount inspection"
        );
        let after = observe_same_mount(&self.0.file, &encoded, parent, entry.physical_identity())?;
        ensure!(
            before == after,
            "private source mount domain changed during inspection"
        );
        ensure!(
            self.physical_identity()? == parent
                && self.private_entry_after_parent_revalidation(name)? == Some(entry),
            "private source identity changed after mount inspection"
        );
        self.revalidate()?;
        Ok((parent, entry))
    }
}

#[cfg(all(target_os = "linux", any(target_env = "gnu", target_env = "musl")))]
type MountDomain = u64;
#[cfg(target_os = "macos")]
type MountDomain = [i32; 2];
#[cfg(not(any(
    target_os = "macos",
    all(target_os = "linux", any(target_env = "gnu", target_env = "musl"))
)))]
type MountDomain = ();

fn observe_same_mount(
    parent: &File,
    name: &CStr,
    parent_identity: (u64, u64),
    entry_identity: (u64, u64),
) -> Result<MountDomain> {
    // Only the internal '.' query bypasses child_name. The caller has already
    // validated the externally supplied entry as one non-traversing component.
    let directory = mount_domain(parent.as_raw_fd(), c".", parent_identity)?;
    let entry = mount_domain(parent.as_raw_fd(), name, entry_identity)?;
    ensure!(
        directory == entry,
        "private source entry has a different mount domain from its owner-lock parent"
    );
    Ok(directory)
}

#[cfg(all(target_os = "linux", any(target_env = "gnu", target_env = "musl")))]
fn linux_statx(parent: RawFd, name: &CStr) -> Result<libc::statx> {
    let mut facts = std::mem::MaybeUninit::<libc::statx>::zeroed();
    // SAFETY: live retained directory FD, validated C component (or internal
    // '.'), and a writable, initialized C output buffer. NOFOLLOW observes the
    // entry itself; NO_AUTOMOUNT never triggers an automount for inspection.
    let status = unsafe {
        libc::statx(
            parent,
            name.as_ptr(),
            libc::AT_SYMLINK_NOFOLLOW | libc::AT_NO_AUTOMOUNT,
            libc::STATX_MNT_ID | libc::STATX_INO | libc::STATX_TYPE,
            facts.as_mut_ptr(),
        )
    };
    if status != 0 {
        let error = std::io::Error::last_os_error();
        return Err(error).context("private source mount inspection failed");
    }
    // SAFETY: the buffer was initialized and statx returned success. Requested
    // field presence is checked independently before interpreting its values.
    Ok(unsafe { facts.assume_init() })
}

#[cfg(all(target_os = "linux", any(target_env = "gnu", target_env = "musl")))]
fn mount_domain(parent: RawFd, name: &CStr, expected: (u64, u64)) -> Result<MountDomain> {
    linux_mount_from_statx(&linux_statx(parent, name)?, expected)
}

#[cfg(all(target_os = "linux", any(target_env = "gnu", target_env = "musl")))]
fn linux_mount_from_statx(facts: &libc::statx, expected: (u64, u64)) -> Result<MountDomain> {
    let required = libc::STATX_MNT_ID | libc::STATX_INO;
    ensure!(
        facts.stx_mask & required == required,
        "private source mount inspection lacks required mount/inode fields"
    );
    #[allow(clippy::unnecessary_cast)]
    let identity = (
        libc::makedev(facts.stx_dev_major, facts.stx_dev_minor) as u64,
        facts.stx_ino,
    );
    ensure!(
        identity == expected,
        "private source identity changed in native mount observation"
    );
    Ok(facts.stx_mnt_id)
}

#[cfg(target_os = "macos")]
fn mount_domain(parent: RawFd, name: &CStr, _expected: (u64, u64)) -> Result<MountDomain> {
    let mut attributes = libc::attrlist {
        bitmapcount: libc::ATTR_BIT_MAP_COUNT,
        reserved: 0,
        commonattr: libc::ATTR_CMN_RETURNED_ATTRS,
        volattr: 0,
        dirattr: 0,
        fileattr: 0,
        forkattr: libc::ATTR_CMNEXT_REALFSID,
    };
    // Native packed layout: uint32 length, five returned attrgroup_t words,
    // then one fsid_t (two int32 words). No optional/variable field is asked.
    let mut response = [0_u8; 32];
    // SAFETY: retained directory FD, validated component (or internal '.'),
    // writable attrlist and bounded byte buffer. Darwin packs bytes; decoding
    // below uses copies, never references or casts to unaligned C structures.
    let status = unsafe {
        libc::getattrlistat(
            parent,
            name.as_ptr(),
            (&mut attributes as *mut libc::attrlist).cast(),
            response.as_mut_ptr().cast(),
            response.len(),
            (libc::FSOPT_ATTR_CMN_EXTENDED | libc::FSOPT_NOFOLLOW) as libc::c_ulong,
        )
    };
    if status != 0 {
        let error = std::io::Error::last_os_error();
        return Err(error).context("private source real mount inspection failed");
    }
    darwin_mount_from_response(&response)
}

#[cfg(target_os = "macos")]
fn darwin_mount_from_response(response: &[u8]) -> Result<MountDomain> {
    ensure!(
        response.len() == 32,
        "private source real mount response has an invalid size"
    );
    let word = |offset| -> Result<u32> {
        let bytes = response
            .get(offset..offset + 4)
            .context("private source real mount response is incomplete")?;
        Ok(u32::from_ne_bytes(bytes.try_into()?))
    };
    ensure!(
        word(0)? == 32,
        "private source real mount response is truncated or unexpected"
    );
    ensure!(
        word(4)? == libc::ATTR_CMN_RETURNED_ATTRS
            && word(8)? == 0
            && word(12)? == 0
            && word(16)? == 0
            && word(20)? == libc::ATTR_CMNEXT_REALFSID,
        "private source real mount response has missing or unexpected attributes"
    );
    // XNU attr_pack_common_extended packs vnode->v_mount->mnt_vfsstat.f_fsid
    // for REALFSID. The ordinary ATTR_CMN_FSID/va_fsid64 path is not accepted.
    Ok([word(24)? as i32, word(28)? as i32])
}

#[cfg(not(any(
    target_os = "macos",
    all(target_os = "linux", any(target_env = "gnu", target_env = "musl"))
)))]
fn mount_domain(_parent: RawFd, _name: &CStr, _expected: (u64, u64)) -> Result<MountDomain> {
    anyhow::bail!("private source mount-domain inspection is unsupported on this platform")
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{
        fs::{self, OpenOptions},
        os::unix::{
            ffi::OsStrExt,
            fs::{MetadataExt, OpenOptionsExt, PermissionsExt, symlink},
        },
    };

    fn private_directory() -> (tempfile::TempDir, Directory) {
        let temporary = tempfile::tempdir().unwrap();
        fs::set_permissions(temporary.path(), fs::Permissions::from_mode(0o700)).unwrap();
        let path = temporary.path().canonicalize().unwrap();
        let directory = Directory::open_existing(&path).unwrap();
        (temporary, directory)
    }

    fn create_private(directory: &Directory, name: &str) -> fs::Metadata {
        let file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(directory.path().join(name))
            .unwrap();
        file.metadata().unwrap()
    }

    #[cfg(any(
        target_os = "macos",
        all(target_os = "linux", any(target_env = "gnu", target_env = "musl"))
    ))]
    #[test]
    fn same_mount_private_source_exposes_actual_parent_and_entry_identities() {
        let (_temporary, directory) = private_directory();
        let parent = fs::metadata(directory.path()).unwrap();
        let entry = create_private(&directory, "source.sqlite");
        assert_eq!(
            directory.physical_identity().unwrap(),
            (parent.dev(), parent.ino())
        );
        assert_eq!(
            directory
                .private_entry_same_mount(OsStr::new("source.sqlite"))
                .unwrap()
                .physical_identity(),
            (entry.dev(), entry.ino())
        );
    }

    #[test]
    fn same_mount_source_refuses_absence_and_unchecked_names() {
        let (_temporary, directory) = private_directory();
        for name in [
            b"absent".as_slice(),
            b"",
            b".",
            b"..",
            b"../source.sqlite",
            b"child/source.sqlite",
            b"nul\0entry",
        ] {
            assert!(
                directory
                    .private_entry_same_mount(OsStr::from_bytes(name))
                    .is_err()
            );
        }
    }

    #[test]
    fn same_mount_source_keeps_private_entry_alias_and_permission_guards() {
        let (_temporary, directory) = private_directory();
        create_private(&directory, "source.sqlite");
        symlink("source.sqlite", directory.path().join("symbolic.sqlite")).unwrap();
        assert!(
            directory
                .private_entry_same_mount(OsStr::new("symbolic.sqlite"))
                .is_err()
        );
        fs::hard_link(
            directory.path().join("source.sqlite"),
            directory.path().join("hard.sqlite"),
        )
        .unwrap();
        for name in ["source.sqlite", "hard.sqlite"] {
            assert!(
                directory
                    .private_entry_same_mount(OsStr::new(name))
                    .is_err()
            );
        }
        create_private(&directory, "shared.sqlite");
        fs::set_permissions(
            directory.path().join("shared.sqlite"),
            fs::Permissions::from_mode(0o640),
        )
        .unwrap();
        assert!(
            directory
                .private_entry_same_mount(OsStr::new("shared.sqlite"))
                .is_err()
        );
    }

    #[test]
    fn physical_identity_and_same_mount_refuse_replaced_parent() {
        let (_temporary, base) = private_directory();
        let directory = base.child("source").unwrap();
        create_private(&directory, "source.sqlite");
        fs::rename(directory.path(), base.path().join("previous")).unwrap();
        base.child("source").unwrap();
        assert!(directory.physical_identity().is_err());
        assert!(
            directory
                .private_entry_same_mount(OsStr::new("source.sqlite"))
                .is_err()
        );
    }

    #[test]
    fn physical_identity_and_same_mount_refuse_parent_permission_change() {
        let (_temporary, directory) = private_directory();
        create_private(&directory, "source.sqlite");
        fs::set_permissions(directory.path(), fs::Permissions::from_mode(0o750)).unwrap();
        assert!(directory.physical_identity().is_err());
        assert!(
            directory
                .private_entry_same_mount(OsStr::new("source.sqlite"))
                .is_err()
        );
    }

    #[cfg(any(
        target_os = "macos",
        all(target_os = "linux", any(target_env = "gnu", target_env = "musl"))
    ))]
    #[test]
    fn native_mount_failure_preserves_typed_errno_through_context_without_private_name() {
        // A relative lookup from -1 must fail EBADF. This exercises the real
        // native error path without opening a source or exhausting resources.
        let error = mount_domain(-1, c"private-source-name-fixture", (0, 0))
            .context("source binding observation unavailable")
            .unwrap_err();
        let cause = error
            .chain()
            .find_map(|cause| cause.downcast_ref::<std::io::Error>());
        assert_eq!(
            cause.and_then(std::io::Error::raw_os_error),
            Some(libc::EBADF)
        );
        assert!(!format!("{error:#}").contains("private-source-name-fixture"));
    }

    #[cfg(all(target_os = "linux", any(target_env = "gnu", target_env = "musl")))]
    #[test]
    fn linux_mount_response_refuses_missing_fields_and_identity_mismatch() {
        // SAFETY: statx is a C data-only output struct; all-zero fields are valid.
        let mut facts: libc::statx = unsafe { std::mem::zeroed() };
        facts.stx_mask = libc::STATX_MNT_ID | libc::STATX_INO;
        facts.stx_mnt_id = 17;
        facts.stx_ino = 56;
        facts.stx_dev_major = 12;
        facts.stx_dev_minor = 34;
        // Linux device encoding for major12/minor34 is (12 << 8) | 34.
        let identity = (3106, 56);
        assert_eq!(linux_mount_from_statx(&facts, identity).unwrap(), 17);
        for mask in [0, libc::STATX_INO, libc::STATX_MNT_ID] {
            facts.stx_mask = mask;
            assert!(linux_mount_from_statx(&facts, identity).is_err());
        }
        facts.stx_mask = libc::STATX_MNT_ID | libc::STATX_INO;
        assert!(linux_mount_from_statx(&facts, (3106, 57)).is_err());
        assert!(linux_mount_from_statx(&facts, (3107, 56)).is_err());
    }

    #[cfg(all(target_os = "linux", any(target_env = "gnu", target_env = "musl")))]
    #[test]
    fn linux_mount_observation_does_not_follow_a_symlink() {
        let (_temporary, directory) = private_directory();
        create_private(&directory, "source.sqlite");
        symlink("source.sqlite", directory.path().join("alias")).unwrap();
        let alias = fs::symlink_metadata(directory.path().join("alias")).unwrap();
        // This isolates the native no-follow contract. The public source
        // guard separately refuses the symlink before any mount observation.
        assert!(
            mount_domain(
                directory.0.file.as_raw_fd(),
                c"alias",
                (alias.dev(), alias.ino()),
            )
            .is_ok()
        );
    }

    #[cfg(target_os = "macos")]
    fn darwin_response(words: &[u32]) -> Vec<u8> {
        words.iter().flat_map(|word| word.to_ne_bytes()).collect()
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn darwin_realfsid_response_requires_exact_length_and_returned_attributes() {
        let valid = [32, 0x8000_0000, 0, 0, 0, 0x80, 7, (-11_i32) as u32];
        assert_eq!(
            darwin_mount_from_response(&darwin_response(&valid)).unwrap(),
            [7, -11]
        );
        for (index, value) in [
            (0, 28),
            (0, 36),
            (1, 0),
            (1, 0x8000_0001),
            (2, 1),
            (3, 1),
            (4, 1),
            (5, 0),
            (5, 0x81),
        ] {
            let mut incomplete = valid;
            incomplete[index] = value;
            assert!(darwin_mount_from_response(&darwin_response(&incomplete)).is_err());
        }
        let bytes = darwin_response(&valid);
        assert!(darwin_mount_from_response(&bytes[..31]).is_err());
        let mut oversized = bytes;
        oversized.push(0);
        assert!(darwin_mount_from_response(&oversized).is_err());
    }

    #[cfg(all(target_os = "linux", any(target_env = "gnu", target_env = "musl")))]
    #[test]
    #[ignore = "admitted Docker QA only: requires existing distinct /etc/hosts file mount; no mount or file-content access"]
    fn linux_docker_file_mount_is_observed_outside_its_parent_domain() {
        let directory = OpenOptions::new()
            .read(true)
            .custom_flags(libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_CLOEXEC)
            .open("/etc")
            .unwrap();
        // statx only, with no content read or FD to /etc/hosts. This
        // precondition fails if the admitted container lacks the file mount.
        let parent_facts = linux_statx(directory.as_raw_fd(), c".").unwrap();
        let entry_facts = linux_statx(directory.as_raw_fd(), c"hosts").unwrap();
        assert_ne!(entry_facts.stx_mask & libc::STATX_TYPE, 0);
        assert_eq!(entry_facts.stx_mode as u32 & libc::S_IFMT, libc::S_IFREG);
        let parent_identity = (
            libc::makedev(parent_facts.stx_dev_major, parent_facts.stx_dev_minor) as u64,
            parent_facts.stx_ino,
        );
        let entry_identity = (
            libc::makedev(entry_facts.stx_dev_major, entry_facts.stx_dev_minor) as u64,
            entry_facts.stx_ino,
        );
        let parent_mount = linux_mount_from_statx(&parent_facts, parent_identity).unwrap();
        let entry_mount = linux_mount_from_statx(&entry_facts, entry_identity).unwrap();
        assert_ne!(
            parent_mount, entry_mount,
            "QA prerequisite missing: /etc/hosts is not a distinct file mount"
        );
        assert!(observe_same_mount(&directory, c"hosts", parent_identity, entry_identity).is_err());
    }
}
