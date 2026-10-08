//! Path canonicalization and folder scopes (`--here`, `--under`, `--near`, `--all`).

use std::path::{Component, Path, PathBuf};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Scope {
    /// cwd == base
    Here,
    /// cwd == base or a descendant
    Under,
    /// `Under`, plus ancestors of base
    Near,
    All,
}

impl Scope {
    pub fn matches(self, base: &Path, cwd: &Path) -> bool {
        match self {
            Scope::Here => cwd == base,
            Scope::Under => cwd.starts_with(base),
            Scope::Near => cwd.starts_with(base) || base.starts_with(cwd),
            Scope::All => true,
        }
    }

    pub fn flag(self) -> &'static str {
        match self {
            Scope::Here => "--here",
            Scope::Under => "--under",
            Scope::Near => "--near",
            Scope::All => "--all",
        }
    }
}

/// Resolve symlinks when the path exists, otherwise normalize lexically; strip
/// the macOS `/System/Volumes/Data` firmlink prefix and trailing slashes.
pub fn canonical(p: &Path) -> PathBuf {
    let abs = if p.is_absolute() { p.to_path_buf() } else { std::env::current_dir().unwrap_or_default().join(p) };
    let resolved = std::fs::canonicalize(&abs).unwrap_or_else(|_| normalize(&abs));
    match resolved.strip_prefix("/System/Volumes/Data") {
        Ok(rest) => Path::new("/").join(rest),
        Err(_) => resolved,
    }
}

fn normalize(p: &Path) -> PathBuf {
    let mut out = PathBuf::new();
    for c in p.components() {
        match c {
            Component::ParentDir => {
                out.pop();
            }
            Component::CurDir => {}
            other => out.push(other),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scopes() {
        let base = Path::new("/w/a/b");
        assert!(Scope::Here.matches(base, Path::new("/w/a/b")));
        assert!(!Scope::Here.matches(base, Path::new("/w/a/b/c")));
        assert!(Scope::Under.matches(base, Path::new("/w/a/b/c")));
        assert!(!Scope::Under.matches(base, Path::new("/w/a/bc")));
        assert!(Scope::Near.matches(base, Path::new("/w/a")));
        assert!(!Scope::Near.matches(base, Path::new("/w/x")));
    }

    #[test]
    fn normalizes_missing_paths() {
        assert_eq!(canonical(Path::new("/nonexistent-cal/a/../b/")), PathBuf::from("/nonexistent-cal/b"));
    }
}
