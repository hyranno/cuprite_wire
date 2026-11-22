//! Temporary directories for runs and builds.

use std::{
    fs, io,
    path::{Path, PathBuf},
    sync::atomic::{AtomicUsize, Ordering},
};

/// A new, empty directory, removed with its contents when dropped unless kept.
#[derive(Debug)]
pub(crate) struct TempDir {
    /// `None` once kept.
    path: Option<PathBuf>,
}

impl TempDir {
    /// A new directory `cuprite_wire-<label>-<pid>-<n>` in the temporary directory of the system.
    pub(crate) fn new(label: &str) -> io::Result<Self> {
        Self::new_in(
            &std::env::temp_dir(),
            &format!("{}-{label}", env!("CARGO_PKG_NAME")),
        )
    }

    /// A new directory `<prefix>-<pid>-<n>` in `parent`, which must exist.
    pub(crate) fn new_in(parent: &Path, prefix: &str) -> io::Result<Self> {
        static COUNTER: AtomicUsize = AtomicUsize::new(0);
        loop {
            let n = COUNTER.fetch_add(1, Ordering::Relaxed);
            let path = parent.join(format!("{prefix}-{}-{n}", std::process::id()));
            match fs::create_dir(&path) {
                Ok(()) => return Ok(TempDir { path: Some(path) }),
                Err(e) if e.kind() == io::ErrorKind::AlreadyExists => continue,
                Err(e) => return Err(e),
            }
        }
    }

    pub(crate) fn path(&self) -> &Path {
        self.path.as_deref().expect("the directory is not kept yet")
    }

    /// The path of the directory, which is no longer removed.
    pub(crate) fn keep(mut self) -> PathBuf {
        self.path.take().expect("the directory is not kept yet")
    }
}

impl std::ops::Deref for TempDir {
    type Target = Path;

    fn deref(&self) -> &Path {
        self.path()
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        if let Some(path) = &self.path {
            let _ = fs::remove_dir_all(path);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_removed_unless_kept() {
        let dir = TempDir::new("test").unwrap();
        let other = TempDir::new("test").unwrap();
        assert_ne!(dir.path(), other.path());
        let path = dir.to_path_buf();
        fs::write(path.join("file"), "x").unwrap();
        drop(dir);
        assert!(!path.exists());

        let path = other.keep();
        assert!(path.is_dir());
        fs::remove_dir(&path).unwrap();
    }
}
