//! Exclusive directory publication for newly prepared local configuration.
//! No existing file/directory is repaired, overwritten or removed on failure.
use super::*;
#[cfg(test)]
thread_local! {pub(crate) static FAIL_AFTER_RENAME:std::cell::Cell<bool>=const{std::cell::Cell::new(false)};}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum PublicationFailure {
    NotPublished,
    DurabilityUnknown,
}

impl Directory {
    pub(crate) fn require_absent_child(&self, name: &str) -> Result<()> {
        self.revalidate()?;
        let name = child_name(OsStr::new(name))?;
        let mut facts = std::mem::MaybeUninit::<libc::stat>::uninit();
        // SAFETY: checked parent FD, one component and writable metadata.
        let result = unsafe {
            libc::fstatat(
                self.0.file.as_raw_fd(),
                name.as_ptr(),
                facts.as_mut_ptr(),
                libc::AT_SYMLINK_NOFOLLOW,
            )
        };
        ensure!(
            result != 0 && std::io::Error::last_os_error().raw_os_error() == Some(libc::ENOENT),
            "exclusive final destination must be absent"
        );
        self.revalidate()
    }
    pub(crate) fn exclusive_child(&self, name: &str) -> Result<Self> {
        self.revalidate()?;
        let encoded = child_name(OsStr::new(name))?;
        // SAFETY: retained checked parent and one validated component. EEXIST
        // is always refusal; no existing staging namespace is adopted.
        ensure!(
            unsafe { libc::mkdirat(self.0.file.as_raw_fd(), encoded.as_ptr(), 0o700) } == 0,
            "new private directory creation refused"
        );
        let file = open_child_directory(&self.0.file, &encoded, false)?;
        let facts = check_directory(&file, true)?;
        let mut chain = self.0.chain.clone();
        chain.push(Identity::of(&facts));
        let child = Self(Arc::new(Anchor {
            file,
            path: self.path().join(name),
            chain,
            private: true,
        }));
        child.revalidate()?;
        child.sync()?;
        self.sync()?;
        Ok(child)
    }

    pub(crate) fn publish_child_exclusive(
        &self,
        staged: &Directory,
        target: &str,
    ) -> std::result::Result<(), PublicationFailure> {
        let before = || -> Result<()> {
            self.revalidate()?;
            staged.revalidate()?;
            ensure!(
                staged.path().parent() == Some(self.path()),
                "publication staging parent differs"
            );
            staged.sync()?;
            Ok(())
        };
        before().map_err(|_| PublicationFailure::NotPublished)?;
        let source = child_name(
            staged
                .path()
                .file_name()
                .ok_or(PublicationFailure::NotPublished)?,
        )
        .map_err(|_| PublicationFailure::NotPublished)?;
        let target =
            child_name(OsStr::new(target)).map_err(|_| PublicationFailure::NotPublished)?;
        // SAFETY: retained parent FD and checked single-component names;
        // the platform flag forbids replacing any existing target entry.
        #[cfg(target_os = "linux")]
        let renamed = unsafe {
            libc::renameat2(
                self.0.file.as_raw_fd(),
                source.as_ptr(),
                self.0.file.as_raw_fd(),
                target.as_ptr(),
                libc::RENAME_NOREPLACE,
            )
        };
        #[cfg(target_os = "macos")]
        let renamed = unsafe {
            libc::renameatx_np(
                self.0.file.as_raw_fd(),
                source.as_ptr(),
                self.0.file.as_raw_fd(),
                target.as_ptr(),
                libc::RENAME_EXCL,
            )
        };
        #[cfg(not(any(target_os = "linux", target_os = "macos")))]
        let renamed = -1;
        if renamed != 0 {
            return Err(PublicationFailure::NotPublished);
        }
        #[cfg(test)]
        if FAIL_AFTER_RENAME.with(|flag| flag.replace(false)) {
            return Err(PublicationFailure::DurabilityUnknown);
        }
        let validate_final = || -> Result<()> {
            self.revalidate()?;
            let current = open_child_directory(&self.0.file, &target, false)?;
            let facts = check_directory(&current, true)?;
            ensure!(
                Identity::of(&facts) == Identity::of(&staged.0.file.metadata()?),
                "published directory identity changed"
            );
            Ok(())
        };
        validate_final().map_err(|_| PublicationFailure::DurabilityUnknown)?;
        // From this point onward an error is never called rollback. The final
        // namespace remains present and normal activation must be refused.
        self.sync()
            .map_err(|_| PublicationFailure::DurabilityUnknown)?;
        Ok(())
    }
}
