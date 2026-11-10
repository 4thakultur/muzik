//! Library roots and path containment checks.
//!
//! Clients never send paths. Paths come from the database (written by the
//! scanner) and are re-validated here before any file is opened.

use std::path::{Path, PathBuf};

#[derive(Debug, Clone)]
pub struct LibraryRoots {
    roots: Vec<PathBuf>,
}

#[derive(Debug, thiserror::Error)]
pub enum PathError {
    #[error("path is outside every library root")]
    OutsideLibrary,
    #[error(transparent)]
    Io(#[from] std::io::Error),
}

impl LibraryRoots {
    /// Canonicalizes each root. Missing roots are skipped with a warning so a
    /// temporarily unmounted drive doesn't prevent startup.
    pub fn new(paths: &[PathBuf]) -> Self {
        let roots = paths
            .iter()
            .filter_map(|p| match std::fs::canonicalize(p) {
                Ok(c) => Some(c),
                Err(e) => {
                    tracing::warn!(path = %p.display(), error = %e, "library root unavailable");
                    None
                }
            })
            .collect();
        Self { roots }
    }

    pub fn roots(&self) -> &[PathBuf] {
        &self.roots
    }

    /// Canonicalizes `path` (resolving symlinks and `..`) and returns it only if
    /// it lies inside one of the library roots.
    pub fn resolve(&self, path: &Path) -> Result<PathBuf, PathError> {
        let canonical = std::fs::canonicalize(path)?;
        if self.contains(&canonical) {
            Ok(canonical)
        } else {
            Err(PathError::OutsideLibrary)
        }
    }

    /// `Path::starts_with` compares whole components, so `/music2` is not
    /// considered inside `/music`.
    pub fn contains(&self, canonical: &Path) -> bool {
        self.roots.iter().any(|r| canonical.starts_with(r))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_traversal_and_sibling_dirs() {
        let tmp = tempfile::tempdir().unwrap();
        let lib = tmp.path().join("music");
        let sibling = tmp.path().join("music2");
        std::fs::create_dir_all(lib.join("a")).unwrap();
        std::fs::create_dir_all(&sibling).unwrap();
        std::fs::write(lib.join("a/song.flac"), b"x").unwrap();
        std::fs::write(sibling.join("secret.flac"), b"x").unwrap();
        std::fs::write(tmp.path().join("passwd"), b"x").unwrap();

        let roots = LibraryRoots::new(std::slice::from_ref(&lib));
        assert!(roots.resolve(&lib.join("a/song.flac")).is_ok());
        assert!(matches!(
            roots.resolve(&lib.join("a/../../passwd")),
            Err(PathError::OutsideLibrary)
        ));
        assert!(matches!(
            roots.resolve(&sibling.join("secret.flac")),
            Err(PathError::OutsideLibrary)
        ));
        assert!(roots.resolve(&lib.join("missing.flac")).is_err());
    }

    #[cfg(unix)]
    #[test]
    fn rejects_symlink_escaping_root() {
        let tmp = tempfile::tempdir().unwrap();
        let lib = tmp.path().join("music");
        std::fs::create_dir_all(&lib).unwrap();
        std::fs::write(tmp.path().join("outside.flac"), b"x").unwrap();
        std::os::unix::fs::symlink(tmp.path().join("outside.flac"), lib.join("link.flac")).unwrap();

        let roots = LibraryRoots::new(std::slice::from_ref(&lib));
        assert!(matches!(
            roots.resolve(&lib.join("link.flac")),
            Err(PathError::OutsideLibrary)
        ));
    }
}
