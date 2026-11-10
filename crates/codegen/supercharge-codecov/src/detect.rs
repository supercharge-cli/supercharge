//! Project-kind detection from marker files. Pure filesystem inspection; no processes.

use std::path::Path;

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum ProjectKind {
    #[default]
    Unknown,
    Rust,
    Go,
    Python,
    JavaScript,
}

impl ProjectKind {
    pub fn label(self) -> &'static str {
        match self {
            Self::Rust => "Rust",
            Self::Go => "Go",
            Self::Python => "Python",
            Self::JavaScript => "JavaScript/TypeScript",
            Self::Unknown => "unknown",
        }
    }
}

/// Detect the project kind from marker files at `root`, most specific first.
pub fn detect_project(root: &Path) -> ProjectKind {
    if root.join("Cargo.toml").is_file() {
        return ProjectKind::Rust;
    }
    if root.join("go.mod").is_file() {
        return ProjectKind::Go;
    }
    if root.join("pyproject.toml").is_file()
        || root.join("setup.py").is_file()
        || root.join("requirements.txt").is_file()
    {
        return ProjectKind::Python;
    }
    if root.join("package.json").is_file() {
        return ProjectKind::JavaScript;
    }
    ProjectKind::Unknown
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn temp_root(name: &str, marker: &str) -> std::path::PathBuf {
        let root =
            std::env::temp_dir().join(format!("codecov-detect-{name}-{}", std::process::id()));
        fs::create_dir_all(&root).unwrap();
        if !marker.is_empty() {
            fs::write(root.join(marker), "").unwrap();
        }
        root
    }

    #[test]
    fn detects_rust_from_cargo_toml() {
        let root = temp_root("rust", "Cargo.toml");
        assert_eq!(detect_project(&root), ProjectKind::Rust);
        fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn detects_go_from_go_mod() {
        let root = temp_root("go", "go.mod");
        assert_eq!(detect_project(&root), ProjectKind::Go);
        fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn detects_python_from_pyproject() {
        let root = temp_root("py", "pyproject.toml");
        assert_eq!(detect_project(&root), ProjectKind::Python);
        fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn detects_javascript_from_package_json() {
        let root = temp_root("js", "package.json");
        assert_eq!(detect_project(&root), ProjectKind::JavaScript);
        fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn unknown_when_no_markers() {
        let root = temp_root("none", "");
        assert_eq!(detect_project(&root), ProjectKind::Unknown);
        fs::remove_dir_all(&root).ok();
    }
}
