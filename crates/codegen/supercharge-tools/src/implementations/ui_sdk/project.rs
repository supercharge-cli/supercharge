//! Detects a frontend project in the workspace: its UI framework, package manager and scripts.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

/// UI framework with a UI SDK adapter.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Framework {
    React,
    Vue,
    Svelte,
    Solid,
    WebComponents,
}

impl Framework {
    pub fn label(self) -> &'static str {
        match self {
            Framework::React => "React",
            Framework::Vue => "Vue",
            Framework::Svelte => "Svelte",
            Framework::Solid => "Solid",
            Framework::WebComponents => "Web Components",
        }
    }

    /// The `@ui-sdk/*` adapter package for this framework.
    pub fn adapter(self) -> &'static str {
        match self {
            Framework::React => "@ui-sdk/react",
            Framework::Vue => "@ui-sdk/vue",
            Framework::Svelte => "@ui-sdk/svelte",
            Framework::Solid => "@ui-sdk/solid",
            Framework::WebComponents => "@ui-sdk/web",
        }
    }
}

/// Dependencies that mark each framework. Checked against dependencies, devDependencies and peerDependencies.
const FRAMEWORK_MARKERS: &[(Framework, &[&str])] = &[
    (
        Framework::React,
        &[
            "react-dom",
            "next",
            "@remix-run/react",
            "gatsby",
            "@tanstack/react-start",
            "@ui-sdk/react",
        ],
    ),
    (Framework::Vue, &["vue", "nuxt", "@ui-sdk/vue"]),
    (
        Framework::Svelte,
        &["svelte", "@sveltejs/kit", "@ui-sdk/svelte"],
    ),
    (
        Framework::Solid,
        &["solid-js", "@solidjs/start", "@ui-sdk/solid"],
    ),
    (
        Framework::WebComponents,
        &["lit", "@stencil/core", "@ui-sdk/web"],
    ),
];

/// Frontend tooling without one of the five adapters; these projects use Web Components.
const OTHER_FRONTEND_MARKERS: &[&str] = &[
    "@angular/core",
    "preact",
    "astro",
    "@builder.io/qwik",
    "vite",
    "parcel",
];

/// Package manager, chosen from the lockfile.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PackageManager {
    Bun,
    Pnpm,
    Yarn,
    Npm,
}

impl PackageManager {
    pub fn name(self) -> &'static str {
        match self {
            PackageManager::Bun => "bun",
            PackageManager::Pnpm => "pnpm",
            PackageManager::Yarn => "yarn",
            PackageManager::Npm => "npm",
        }
    }

    /// Lockfiles in priority order, with the manager each one means.
    const LOCKFILES: &'static [(&'static str, PackageManager)] = &[
        ("bun.lock", PackageManager::Bun),
        ("bun.lockb", PackageManager::Bun),
        ("pnpm-lock.yaml", PackageManager::Pnpm),
        ("yarn.lock", PackageManager::Yarn),
        ("package-lock.json", PackageManager::Npm),
        ("npm-shrinkwrap.json", PackageManager::Npm),
    ];

    fn from_lockfile_in(dir: &Path) -> Option<Self> {
        Self::LOCKFILES
            .iter()
            .find(|(name, _)| dir.join(name).is_file())
            .map(|(_, pm)| *pm)
    }

    /// Parse the `packageManager` field, e.g. `pnpm@9.1.0`.
    fn from_field(value: &str) -> Option<Self> {
        let name = value.split('@').next().unwrap_or_default().trim();
        match name {
            "bun" => Some(PackageManager::Bun),
            "pnpm" => Some(PackageManager::Pnpm),
            "yarn" => Some(PackageManager::Yarn),
            "npm" => Some(PackageManager::Npm),
            _ => None,
        }
    }

    /// Argument vector that adds `packages` as dependencies.
    pub fn add_args(self, packages: &[String]) -> Vec<String> {
        let verb = match self {
            PackageManager::Npm => "install",
            _ => "add",
        };
        let mut args = vec![verb.to_string()];
        args.extend(packages.iter().cloned());
        args
    }

    /// Argument vector that runs a `package.json` script.
    pub fn run_script_args(self, script: &str) -> Vec<String> {
        vec!["run".to_string(), script.to_string()]
    }

    /// Program and arguments that run a binary from `node_modules/.bin`.
    pub fn exec(self, bin: &str, args: &[&str]) -> (String, Vec<String>) {
        let mut out: Vec<String> = Vec::new();
        let program = match self {
            PackageManager::Bun => {
                out.push("x".into());
                "bun"
            }
            PackageManager::Pnpm => {
                out.push("exec".into());
                "pnpm"
            }
            PackageManager::Yarn => {
                out.push("run".into());
                "yarn"
            }
            PackageManager::Npm => {
                out.push("--no-install".into());
                "npx"
            }
        };
        out.push(bin.to_string());
        out.extend(args.iter().map(|a| (*a).to_string()));
        (program.to_string(), out)
    }
}

/// A command the UI tools run, with what it checks.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProjectCommand {
    /// `typecheck`, `build` or `lint`.
    pub kind: &'static str,
    pub program: String,
    pub args: Vec<String>,
}

impl ProjectCommand {
    pub fn display(&self) -> String {
        let mut out = self.program.clone();
        for arg in &self.args {
            out.push(' ');
            out.push_str(arg);
        }
        out
    }
}

/// A frontend project found in the workspace.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FrontendProject {
    /// Folder holding the project's `package.json`.
    pub root: PathBuf,
    pub frameworks: Vec<Framework>,
    pub package_manager: PackageManager,
    /// Whether any `@ui-sdk/*` package is declared or installed.
    pub has_ui_sdk: bool,
    pub scripts: BTreeMap<String, String>,
    pub has_tsconfig: bool,
    /// Root `tsconfig.json` only references other configs (`"files": []` plus `references`).
    pub tsconfig_is_solution: bool,
}

/// Scripts that typecheck, most specific first.
const TYPECHECK_SCRIPTS: &[&str] = &[
    "typecheck",
    "type-check",
    "check-types",
    "types",
    "tsc",
    "check",
];

impl FrontendProject {
    /// `@ui-sdk/core` plus one adapter per framework.
    pub fn ui_sdk_packages(&self) -> Vec<String> {
        let mut packages = vec!["@ui-sdk/core".to_string()];
        for framework in &self.frameworks {
            let adapter = framework.adapter().to_string();
            if !packages.contains(&adapter) {
                packages.push(adapter);
            }
        }
        packages
    }

    /// Install command for the UI SDK packages, run in [`Self::root`].
    pub fn install_command(&self) -> ProjectCommand {
        ProjectCommand {
            kind: "install",
            program: self.package_manager.name().to_string(),
            args: self.package_manager.add_args(&self.ui_sdk_packages()),
        }
    }

    pub fn frameworks_label(&self) -> String {
        self.frameworks
            .iter()
            .map(|f| f.label())
            .collect::<Vec<_>>()
            .join(" + ")
    }

    fn script(&self, kind: &'static str, name: &str) -> ProjectCommand {
        ProjectCommand {
            kind,
            program: self.package_manager.name().to_string(),
            args: self.package_manager.run_script_args(name),
        }
    }

    /// The typecheck command: a typecheck script, else the framework's checker when TypeScript is configured.
    pub fn typecheck_command(&self) -> Option<ProjectCommand> {
        if let Some(name) = TYPECHECK_SCRIPTS
            .iter()
            .find(|name| self.scripts.contains_key(**name))
        {
            return Some(self.script("typecheck", name));
        }
        if !self.has_tsconfig {
            return None;
        }
        let (program, args) = if self.frameworks.contains(&Framework::Vue) {
            self.package_manager.exec("vue-tsc", &["--noEmit"])
        } else if self.frameworks.contains(&Framework::Svelte) {
            self.package_manager.exec("svelte-check", &[])
        } else if self.tsconfig_is_solution {
            self.package_manager.exec("tsc", &["-b"])
        } else {
            self.package_manager.exec("tsc", &["--noEmit"])
        };
        Some(ProjectCommand {
            kind: "typecheck",
            program,
            args,
        })
    }

    pub fn build_command(&self) -> Option<ProjectCommand> {
        self.scripts
            .contains_key("build")
            .then(|| self.script("build", "build"))
    }

    pub fn lint_command(&self) -> Option<ProjectCommand> {
        self.scripts
            .contains_key("lint")
            .then(|| self.script("lint", "lint"))
    }

    /// Typecheck then build, the order the UI verification loop runs them.
    pub fn verify_commands(&self) -> Vec<ProjectCommand> {
        self.typecheck_command()
            .into_iter()
            .chain(self.build_command())
            .collect()
    }

    /// The project folder relative to `workspace`, or `None` when they are the same folder
    /// or the project sits outside it.
    pub fn relative_root(&self, workspace: &Path) -> Option<String> {
        let rel = self.root.strip_prefix(workspace).ok()?;
        if rel.as_os_str().is_empty() {
            return None;
        }
        Some(rel.to_string_lossy().replace('\\', "/"))
    }
}

/// How far detection looks above and below the workspace.
const MAX_ANCESTORS: usize = 3;
const MAX_LOCKFILE_ANCESTORS: usize = 6;
const MAX_CHILD_DIRS: usize = 200;
const MAX_PACKAGE_JSON_BYTES: u64 = 2 * 1024 * 1024;

fn read_package_json(dir: &Path) -> Option<serde_json::Value> {
    let path = dir.join("package.json");
    let meta = std::fs::metadata(&path).ok()?;
    if !meta.is_file() || meta.len() > MAX_PACKAGE_JSON_BYTES {
        return None;
    }
    let text = std::fs::read_to_string(&path).ok()?;
    serde_json::from_str(&text).ok()
}

fn dependency_names(package: &serde_json::Value) -> Vec<String> {
    ["dependencies", "devDependencies", "peerDependencies"]
        .iter()
        .filter_map(|key| package.get(key)?.as_object())
        .flat_map(|deps| deps.keys().cloned())
        .collect()
}

/// Frameworks a `package.json` declares, in a stable order. Empty when it is not a frontend project.
pub fn frameworks_in(package: &serde_json::Value) -> Vec<Framework> {
    let deps = dependency_names(package);
    let has = |name: &str| deps.iter().any(|d| d == name);
    let mut found: Vec<Framework> = FRAMEWORK_MARKERS
        .iter()
        .filter(|(_, markers)| markers.iter().any(|m| has(m)))
        .map(|(framework, _)| *framework)
        .collect();
    // A bare `react` without `react-native` is a web React app too.
    if !found.contains(&Framework::React) && has("react") && !has("react-native") {
        found.push(Framework::React);
    }
    if found.is_empty() && OTHER_FRONTEND_MARKERS.iter().any(|m| has(m)) {
        found.push(Framework::WebComponents);
    }
    found.sort();
    found
}

fn tsconfig_facts(dir: &Path) -> (bool, bool) {
    let path = dir.join("tsconfig.json");
    let Ok(text) = std::fs::read_to_string(&path) else {
        return (false, false);
    };
    // tsconfig allows comments and trailing commas, so look for the shape instead of parsing.
    let compact: String = text.chars().filter(|c| !c.is_whitespace()).collect();
    let solution = compact.contains("\"files\":[]") && compact.contains("\"references\":");
    (true, solution)
}

fn package_manager_for(root: &Path, package: &serde_json::Value) -> PackageManager {
    for dir in root.ancestors().take(MAX_LOCKFILE_ANCESTORS + 1) {
        if let Some(pm) = PackageManager::from_lockfile_in(dir) {
            return pm;
        }
        if let Some(pm) = read_package_json(dir)
            .as_ref()
            .or(if dir == root { Some(package) } else { None })
            .and_then(|p| p.get("packageManager")?.as_str().map(str::to_owned))
            .and_then(|field| PackageManager::from_field(&field))
        {
            return pm;
        }
    }
    PackageManager::Npm
}

fn has_installed_ui_sdk(root: &Path) -> bool {
    root.ancestors()
        .take(MAX_LOCKFILE_ANCESTORS + 1)
        .any(|dir| dir.join("node_modules").join("@ui-sdk").is_dir())
}

fn project_at(dir: &Path) -> Option<FrontendProject> {
    let package = read_package_json(dir)?;
    let frameworks = frameworks_in(&package);
    if frameworks.is_empty() {
        return None;
    }
    let scripts = package
        .get("scripts")
        .and_then(|s| s.as_object())
        .map(|s| {
            s.iter()
                .filter_map(|(k, v)| Some((k.clone(), v.as_str()?.to_string())))
                .collect()
        })
        .unwrap_or_default();
    let declared_ui_sdk = dependency_names(&package)
        .iter()
        .any(|d| d.starts_with("@ui-sdk/"));
    let (has_tsconfig, tsconfig_is_solution) = tsconfig_facts(dir);
    Some(FrontendProject {
        root: dir.to_path_buf(),
        package_manager: package_manager_for(dir, &package),
        has_ui_sdk: declared_ui_sdk || has_installed_ui_sdk(dir),
        frameworks,
        scripts,
        has_tsconfig,
        tsconfig_is_solution,
    })
}

fn child_dirs(dir: &Path) -> Vec<PathBuf> {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut dirs: Vec<PathBuf> = entries
        .filter_map(Result::ok)
        .filter(|e| e.file_type().is_ok_and(|t| t.is_dir()))
        .filter(|e| {
            let name = e.file_name();
            let name = name.to_string_lossy();
            !name.starts_with('.') && name != "node_modules" && name != "target"
        })
        .map(|e| e.path())
        .take(MAX_CHILD_DIRS)
        .collect();
    dirs.sort();
    dirs
}

/// Find the frontend project for `workspace`: the workspace itself, then its nearest ancestors
/// (inside a monorepo package), then its direct children and `apps/*`, `packages/*` (a monorepo root).
pub fn detect(workspace: &Path) -> Option<FrontendProject> {
    for dir in workspace.ancestors().take(MAX_ANCESTORS + 1) {
        if let Some(project) = project_at(dir) {
            return Some(project);
        }
    }
    let mut candidates = child_dirs(workspace);
    for group in ["apps", "packages"] {
        candidates.extend(child_dirs(&workspace.join(group)));
    }
    candidates.into_iter().find_map(|dir| project_at(&dir))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn write(dir: &Path, rel: &str, contents: &str) {
        let path = dir.join(rel);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, contents).unwrap();
    }

    fn package(deps: serde_json::Value, scripts: serde_json::Value) -> String {
        json!({ "name": "app", "dependencies": deps, "scripts": scripts }).to_string()
    }

    #[test]
    fn detects_each_framework_from_its_marker() {
        let cases = [
            (json!({"react": "19", "react-dom": "19"}), Framework::React),
            (json!({"next": "15"}), Framework::React),
            (json!({"vue": "3"}), Framework::Vue),
            (json!({"nuxt": "4"}), Framework::Vue),
            (json!({"svelte": "5"}), Framework::Svelte),
            (json!({"@sveltejs/kit": "2"}), Framework::Svelte),
            (json!({"solid-js": "2"}), Framework::Solid),
            (json!({"lit": "3"}), Framework::WebComponents),
            (json!({"@angular/core": "19"}), Framework::WebComponents),
            (json!({"vite": "6"}), Framework::WebComponents),
        ];
        for (deps, expected) in cases {
            let found = frameworks_in(&json!({ "dependencies": deps.clone() }));
            assert_eq!(found, vec![expected], "{deps}");
        }
    }

    #[test]
    fn non_frontend_packages_are_ignored() {
        assert!(frameworks_in(&json!({"dependencies": {"express": "5"}})).is_empty());
        assert!(
            frameworks_in(&json!({"dependencies": {"react": "19", "react-native": "0.79"}}))
                .is_empty()
        );
        assert!(frameworks_in(&json!({})).is_empty());
    }

    #[test]
    fn dev_and_peer_dependencies_count() {
        let found = frameworks_in(&json!({
            "devDependencies": {"svelte": "5"},
            "peerDependencies": {"react-dom": "19"}
        }));
        assert_eq!(found, vec![Framework::React, Framework::Svelte]);
    }

    #[test]
    fn package_manager_comes_from_the_lockfile() {
        for (lock, expected) in [
            ("bun.lock", "bun add @ui-sdk/core @ui-sdk/react"),
            ("bun.lockb", "bun add @ui-sdk/core @ui-sdk/react"),
            ("pnpm-lock.yaml", "pnpm add @ui-sdk/core @ui-sdk/react"),
            ("yarn.lock", "yarn add @ui-sdk/core @ui-sdk/react"),
            (
                "package-lock.json",
                "npm install @ui-sdk/core @ui-sdk/react",
            ),
        ] {
            let dir = tempfile::tempdir().unwrap();
            write(
                dir.path(),
                "package.json",
                &package(json!({"react-dom": "19"}), json!({})),
            );
            write(dir.path(), lock, "");
            let project = detect(dir.path()).expect("react project");
            assert_eq!(project.install_command().display(), expected, "{lock}");
        }
    }

    #[test]
    fn package_manager_falls_back_to_the_field_then_npm() {
        let dir = tempfile::tempdir().unwrap();
        write(
            dir.path(),
            "package.json",
            &json!({"dependencies": {"vue": "3"}, "packageManager": "pnpm@9.12.0"}).to_string(),
        );
        assert_eq!(
            detect(dir.path()).unwrap().package_manager,
            PackageManager::Pnpm
        );
        let plain = tempfile::tempdir().unwrap();
        write(
            plain.path(),
            "package.json",
            &package(json!({"vue": "3"}), json!({})),
        );
        let project = detect(plain.path()).unwrap();
        assert_eq!(project.package_manager, PackageManager::Npm);
        assert_eq!(
            project.install_command().display(),
            "npm install @ui-sdk/core @ui-sdk/vue"
        );
    }

    #[test]
    fn monorepo_lockfile_above_the_app_wins() {
        let root = tempfile::tempdir().unwrap();
        write(root.path(), "pnpm-lock.yaml", "");
        write(
            root.path(),
            "package.json",
            &json!({"private": true}).to_string(),
        );
        write(
            root.path(),
            "apps/web/package.json",
            &package(json!({"solid-js": "2"}), json!({"build": "vite build"})),
        );
        let from_root = detect(root.path()).expect("finds apps/web from the root");
        assert_eq!(from_root.root, root.path().join("apps/web"));
        assert_eq!(from_root.package_manager, PackageManager::Pnpm);
        assert_eq!(
            from_root.relative_root(root.path()).as_deref(),
            Some("apps/web")
        );
        let nested = root.path().join("apps/web/src/components");
        std::fs::create_dir_all(&nested).unwrap();
        let from_inside = detect(&nested).expect("finds the app from inside it");
        assert_eq!(from_inside.root, root.path().join("apps/web"));
    }

    #[test]
    fn verify_commands_prefer_scripts() {
        let dir = tempfile::tempdir().unwrap();
        write(
            dir.path(),
            "package.json",
            &package(
                json!({"react-dom": "19"}),
                json!({"typecheck": "tsc --noEmit", "build": "next build", "lint": "eslint ."}),
            ),
        );
        write(dir.path(), "yarn.lock", "");
        let project = detect(dir.path()).unwrap();
        let shown: Vec<String> = project
            .verify_commands()
            .iter()
            .map(|c| c.display())
            .collect();
        assert_eq!(shown, ["yarn run typecheck", "yarn run build"]);
        assert_eq!(project.lint_command().unwrap().display(), "yarn run lint");
    }

    #[test]
    fn verify_commands_fall_back_to_the_framework_checker() {
        let cases = [
            (json!({"vue": "3"}), "npx --no-install vue-tsc --noEmit"),
            (json!({"svelte": "5"}), "npx --no-install svelte-check"),
            (json!({"react-dom": "19"}), "npx --no-install tsc --noEmit"),
        ];
        for (deps, expected) in cases {
            let dir = tempfile::tempdir().unwrap();
            write(dir.path(), "package.json", &package(deps, json!({})));
            write(dir.path(), "tsconfig.json", "{ \"compilerOptions\": {} }");
            let project = detect(dir.path()).unwrap();
            let shown: Vec<String> = project
                .verify_commands()
                .iter()
                .map(|c| c.display())
                .collect();
            assert_eq!(shown, [expected]);
        }
        let solution = tempfile::tempdir().unwrap();
        write(
            solution.path(),
            "package.json",
            &package(json!({"react-dom": "19"}), json!({})),
        );
        write(
            solution.path(),
            "tsconfig.json",
            "{\n  // solution\n  \"files\": [],\n  \"references\": [{ \"path\": \"./tsconfig.app.json\" }],\n}",
        );
        write(solution.path(), "pnpm-lock.yaml", "");
        let project = detect(solution.path()).unwrap();
        assert_eq!(
            project.typecheck_command().unwrap().display(),
            "pnpm exec tsc -b"
        );
        let no_ts = tempfile::tempdir().unwrap();
        write(
            no_ts.path(),
            "package.json",
            &package(json!({"lit": "3"}), json!({})),
        );
        assert!(detect(no_ts.path()).unwrap().verify_commands().is_empty());
    }

    #[test]
    fn exec_uses_each_managers_runner() {
        assert_eq!(
            PackageManager::Bun.exec("tsc", &[]),
            ("bun".into(), vec!["x".into(), "tsc".into()])
        );
        assert_eq!(
            PackageManager::Pnpm.exec("tsc", &["-b"]),
            (
                "pnpm".into(),
                vec!["exec".into(), "tsc".into(), "-b".into()]
            )
        );
        assert_eq!(PackageManager::Yarn.exec("tsc", &[]).1, vec!["run", "tsc"]);
    }

    #[test]
    fn ui_sdk_presence_is_detected_from_deps_or_node_modules() {
        let declared = tempfile::tempdir().unwrap();
        write(
            declared.path(),
            "package.json",
            &package(
                json!({"react-dom": "19", "@ui-sdk/react": "0.1.0"}),
                json!({}),
            ),
        );
        assert!(detect(declared.path()).unwrap().has_ui_sdk);
        let installed = tempfile::tempdir().unwrap();
        write(
            installed.path(),
            "package.json",
            &package(json!({"vue": "3"}), json!({})),
        );
        assert!(!detect(installed.path()).unwrap().has_ui_sdk);
        std::fs::create_dir_all(installed.path().join("node_modules/@ui-sdk/core")).unwrap();
        assert!(detect(installed.path()).unwrap().has_ui_sdk);
    }

    #[test]
    fn multiple_frameworks_get_one_adapter_each() {
        let dir = tempfile::tempdir().unwrap();
        write(
            dir.path(),
            "package.json",
            &package(
                json!({"react-dom": "19", "vue": "3", "astro": "5"}),
                json!({}),
            ),
        );
        let project = detect(dir.path()).unwrap();
        assert_eq!(project.frameworks_label(), "React + Vue");
        assert_eq!(
            project.ui_sdk_packages(),
            ["@ui-sdk/core", "@ui-sdk/react", "@ui-sdk/vue"]
        );
    }

    #[test]
    fn non_frontend_and_broken_workspaces_detect_nothing() {
        let empty = tempfile::tempdir().unwrap();
        assert!(detect(empty.path()).is_none());
        let rust = tempfile::tempdir().unwrap();
        write(rust.path(), "Cargo.toml", "[package]\nname = \"x\"\n");
        assert!(detect(rust.path()).is_none());
        let broken = tempfile::tempdir().unwrap();
        write(broken.path(), "package.json", "{ not json");
        assert!(detect(broken.path()).is_none());
        let hidden = tempfile::tempdir().unwrap();
        write(
            hidden.path(),
            "node_modules/some-lib/package.json",
            &package(json!({"react-dom": "19"}), json!({})),
        );
        assert!(
            detect(hidden.path()).is_none(),
            "node_modules must not count"
        );
        assert!(detect(Path::new("/definitely/not/here")).is_none());
    }
}
