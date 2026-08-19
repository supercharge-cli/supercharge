//! Template builder core — `Template`, `RegistryConfig`, and `BuildOptions`.
//!
//! This module provides the public `Template` struct and its builder methods
//! for constructing a template build request. It also defines
//! [`RegistryConfig`] for private registry authentication and [`BuildOptions`]
//! for per-build overrides.
//!
//! # Builder chain example
//!
//! ```rust
//! use e2b_rs::template::{Template, BuildOptions};
//! use e2b_rs::wait_for_timeout;
//!
//! let _template = Template::new()
//!     .from_base_image()
//!     .set_start_cmd("npm start", wait_for_timeout(1_000));
//! ```
//!
//! # Serialisation
//!
//! `Template::serialize` and `Template::instruction_steps_from` are
//! `pub(crate)` helpers called by the HTTP-build layer (Tasks 2–4) to produce
//! the wire-format body for the build API.

use crate::errors::Result;
use crate::template::ReadyCmd;
use crate::template::dockerfile::DockerfileAction;
use crate::template::types::{Instruction, InstructionType};

// ──────────────────────────────────────────────────────────────────────────────
// RegistryConfig
// ──────────────────────────────────────────────────────────────────────────────

/// Credentials for pulling the base image from a private container registry.
///
/// Stored on a [`Template`] instance and wired to the build request via the
/// `Template::with_registry` builder (Plan 5d).
///
/// # Secret handling
///
/// Credential fields are intentionally excluded from the `Debug` output —
/// printing `{:?}` shows `<redacted>` in place of any sensitive value.
/// This matches the convention used by [`crate::volume::VolumeOpts`].
#[derive(Clone)]
pub enum RegistryConfig {
    /// Amazon ECR credentials.
    Aws {
        /// AWS Access Key ID.
        access_key_id: String,
        /// AWS Secret Access Key.
        secret_access_key: String,
        /// AWS region where the ECR registry is located (e.g. `"us-east-1"`).
        region: String,
    },
    /// Google Container Registry / Artifact Registry credentials.
    Gcp {
        /// Service-account JSON key (as a raw JSON string).
        service_account_json: String,
    },
    /// Generic Docker Hub / OCI registry credentials.
    General {
        /// Registry username.
        username: String,
        /// Registry password or personal access token.
        password: String,
    },
}

impl std::fmt::Debug for RegistryConfig {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Aws { region, .. } => f
                .debug_struct("RegistryConfig::Aws")
                .field("access_key_id", &"<redacted>")
                .field("secret_access_key", &"<redacted>")
                .field("region", region)
                .finish(),
            Self::Gcp { .. } => f
                .debug_struct("RegistryConfig::Gcp")
                .field("service_account_json", &"<redacted>")
                .finish(),
            Self::General { username, .. } => f
                .debug_struct("RegistryConfig::General")
                .field("username", username)
                .field("password", &"<redacted>")
                .finish(),
        }
    }
}

impl RegistryConfig {
    /// Converts this [`RegistryConfig`] into the wire-format
    /// [`crate::api::schema::FromImageRegistry`] enum.
    ///
    /// Used internally by [`Template::serialize`] to build the API request
    /// body. The generated type is never exposed in the public API.
    pub(crate) fn to_wire(&self) -> crate::api::schema::FromImageRegistry {
        use crate::api::schema::{
            AwsRegistry, AwsRegistryType, FromImageRegistry, GcpRegistry, GcpRegistryType,
            GeneralRegistry, GeneralRegistryType,
        };

        match self {
            Self::Aws {
                access_key_id,
                secret_access_key,
                region,
            } => FromImageRegistry::AwsRegistry(AwsRegistry {
                aws_access_key_id: access_key_id.clone(),
                aws_secret_access_key: secret_access_key.clone(),
                aws_region: region.clone(),
                type_: AwsRegistryType::Aws,
            }),
            Self::Gcp {
                service_account_json,
            } => FromImageRegistry::GcpRegistry(GcpRegistry {
                service_account_json: service_account_json.clone(),
                type_: GcpRegistryType::Gcp,
            }),
            Self::General { username, password } => {
                FromImageRegistry::GeneralRegistry(GeneralRegistry {
                    username: username.clone(),
                    password: password.clone(),
                    type_: GeneralRegistryType::Registry,
                })
            }
        }
    }
}

// ──────────────────────────────────────────────────────────────────────────────
// BuildOptions
// ──────────────────────────────────────────────────────────────────────────────

/// Per-build overrides for a single template build invocation.
///
/// All fields are optional; unset values fall back to the environment-variable
/// resolution performed by [`crate::ConnectionConfig`].
///
/// # Secret handling
///
/// The `api_key` field is excluded from the `Debug` output — it is shown as
/// `<redacted>` to prevent accidental secret leakage in logs.
#[derive(Default, Clone)]
pub struct BuildOptions {
    /// Override the number of vCPUs for the build sandbox.
    pub cpu_count: Option<u32>,
    /// Override the RAM for the build sandbox, in MiB.
    pub memory_mb: Option<u32>,
    /// If `true`, bypass the build cache and force all steps to run.
    pub skip_cache: bool,
    /// Per-request HTTP timeout in milliseconds.
    pub request_timeout_ms: Option<u64>,
    /// Override the E2B API key for this build.
    pub api_key: Option<String>,
    /// Override the E2B domain for this build (e.g. `"e2b.app"`).
    pub domain: Option<String>,
    /// Override the API base URL for this build.
    pub api_url: Option<String>,
}

impl std::fmt::Debug for BuildOptions {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("BuildOptions")
            .field("cpu_count", &self.cpu_count)
            .field("memory_mb", &self.memory_mb)
            .field("skip_cache", &self.skip_cache)
            .field("request_timeout_ms", &self.request_timeout_ms)
            .field(
                "api_key",
                if self.api_key.is_some() {
                    &"<redacted>"
                } else {
                    &"None"
                },
            )
            .field("domain", &self.domain)
            .field("api_url", &self.api_url)
            .finish()
    }
}

// ──────────────────────────────────────────────────────────────────────────────
// File-op builder option structs
// ──────────────────────────────────────────────────────────────────────────────

/// Options for the [`Template::copy`] builder method.
///
/// All fields are optional and default to `None` / `false`. Construct with
/// `Default::default()` or use struct-update syntax:
///
/// ```rust
/// use e2b_rs::template::CopyOpts;
/// let opts = CopyOpts { user: Some("myuser".to_string()), ..Default::default() };
/// ```
#[derive(Default, Clone)]
pub struct CopyOpts {
    /// If `true`, forces the file-upload step even when the content hash
    /// matches the server-side cache.
    pub force_upload: Option<bool>,
    /// User (and optionally group) for the copied files, e.g. `"user:group"`.
    /// Passed verbatim to the build backend's `--chown` equivalent.
    pub user: Option<String>,
    /// Unix file permission bits for the copied files (e.g. `0o755`).
    /// Serialised as a zero-padded four-digit octal string (`"0755"`).
    pub mode: Option<u32>,
    /// Whether to resolve symbolic links in source paths before copying.
    /// When `true`, the target of the symlink is hashed instead of the link
    /// itself.
    pub resolve_symlinks: Option<bool>,
}

/// Options for the [`Template::remove`] builder method.
///
/// Controls the flags passed to `rm` and the user under which the command
/// is executed.
#[derive(Default, Clone)]
pub struct RemoveOpts {
    /// If `true`, pass `-f` to `rm` (suppress errors for non-existent files).
    pub force: bool,
    /// If `true`, pass `-r` to `rm` (remove directories recursively).
    pub recursive: bool,
    /// Run the `rm` command as this user inside the build sandbox.
    pub user: Option<String>,
}

/// Options for the [`Template::rename`] builder method.
///
/// Controls the flags passed to `mv` and the user under which the command
/// is executed.
#[derive(Default, Clone)]
pub struct RenameOpts {
    /// If `true`, pass `-f` to `mv` (overwrite the destination without
    /// prompting).
    pub force: bool,
    /// Run the `mv` command as this user inside the build sandbox.
    pub user: Option<String>,
}

/// Options for the [`Template::make_dir`] builder method.
///
/// Controls the permission mode and the user under which the command is
/// executed.
#[derive(Default, Clone)]
pub struct MakeDirOpts {
    /// Run the `mkdir` command as this user inside the build sandbox.
    pub user: Option<String>,
    /// Unix file permission bits for the created directory (e.g. `0o755`).
    /// Serialised as a zero-padded four-digit octal string and passed via
    /// `-m <mode>` to `mkdir`.
    pub mode: Option<u32>,
}

/// Options for the [`Template::make_symlink`] builder method.
///
/// Controls the flags passed to `ln` and the user under which the command
/// is executed.
#[derive(Default, Clone)]
pub struct MakeSymlinkOpts {
    /// If `true`, pass `-f` to `ln` (remove the destination before creating
    /// the link).
    pub force: bool,
    /// Run the `ln` command as this user inside the build sandbox.
    pub user: Option<String>,
}

/// Options for the [`Template::run_cmd`] and [`Template::run_cmds`] builder
/// methods.
///
/// # Example
///
/// ```rust
/// use e2b_rs::template::{Template, RunCmdOpts};
///
/// let t = Template::new()
///     .run_cmd("echo hello", RunCmdOpts { user: Some("root".to_string()) });
/// ```
#[derive(Default, Clone)]
pub struct RunCmdOpts {
    /// Run the command as this user inside the build sandbox.
    pub user: Option<String>,
}

/// Options for the [`Template::pip_install`] builder method.
///
/// Note: `global` defaults to `true` (matching the JS SDK's `g ?? true`).
/// Use an explicit `PipInstallOpts { global: false }` for user-level installs.
///
/// # Example
///
/// ```rust
/// use e2b_rs::template::{Template, PipInstallOpts};
///
/// // Global install (root, default):
/// let t = Template::new().pip_install(&["requests"], PipInstallOpts::default());
///
/// // Per-user install:
/// let t = Template::new()
///     .pip_install(&["requests"], PipInstallOpts { global: false });
/// ```
#[derive(Clone)]
pub struct PipInstallOpts {
    /// If `true` (the default), run as root and install packages globally.
    /// If `false`, pass `--user` to `pip install` and run without a user override.
    pub global: bool,
}

impl Default for PipInstallOpts {
    fn default() -> Self {
        Self { global: true }
    }
}

/// Options for the [`Template::npm_install`] builder method.
///
/// # Example
///
/// ```rust
/// use e2b_rs::template::{Template, NpmInstallOpts};
///
/// let t = Template::new()
///     .npm_install(&["typescript"], NpmInstallOpts { global: true, dev: false });
/// ```
#[derive(Default, Clone)]
pub struct NpmInstallOpts {
    /// If `true`, pass `-g` to install packages globally (runs as root).
    pub global: bool,
    /// If `true`, pass `--save-dev` to install as a development dependency.
    pub dev: bool,
}

/// Options for the [`Template::bun_install`] builder method.
///
/// # Example
///
/// ```rust
/// use e2b_rs::template::{Template, BunInstallOpts};
///
/// let t = Template::new()
///     .bun_install(&["elysia"], BunInstallOpts { global: false, dev: false });
/// ```
#[derive(Default, Clone)]
pub struct BunInstallOpts {
    /// If `true`, pass `-g` to install packages globally (runs as root).
    pub global: bool,
    /// If `true`, pass `--dev` to install as a development dependency.
    pub dev: bool,
}

/// Options for the [`Template::apt_install`] builder method.
///
/// # Example
///
/// ```rust
/// use e2b_rs::template::{Template, AptInstallOpts};
///
/// let t = Template::new().apt_install(
///     &["curl", "git"],
///     AptInstallOpts { no_install_recommends: true, fix_missing: false },
/// );
/// ```
#[derive(Default, Clone)]
pub struct AptInstallOpts {
    /// If `true`, pass `--no-install-recommends` to `apt-get install`.
    pub no_install_recommends: bool,
    /// If `true`, pass `--fix-missing` to `apt-get install`.
    pub fix_missing: bool,
}

/// Options for the [`Template::git_clone`] builder method.
///
/// # Example
///
/// ```rust
/// use e2b_rs::template::{Template, GitCloneOpts};
///
/// let t = Template::new().git_clone(
///     "https://github.com/owner/repo.git",
///     Some("/opt/repo"),
///     GitCloneOpts { branch: Some("main".to_string()), depth: Some(1), user: None },
/// );
/// ```
#[derive(Default, Clone)]
pub struct GitCloneOpts {
    /// Branch to clone. When set, appends `--branch <branch> --single-branch`.
    pub branch: Option<String>,
    /// Shallow-clone depth. When set, appends `--depth <depth>`.
    pub depth: Option<u32>,
    /// Run the clone command as this user inside the build sandbox.
    pub user: Option<String>,
}

// ──────────────────────────────────────────────────────────────────────────────
// Template
// ──────────────────────────────────────────────────────────────────────────────

/// A template build specification.
///
/// Constructed via [`Template::new`] (or `Default::default()`) and configured
/// through a fluent builder chain. The builder methods consume `self` and
/// return a new `Template`, enabling method chaining without `&mut self`:
///
/// ```rust
/// use e2b_rs::template::Template;
/// use e2b_rs::wait_for_timeout;
///
/// let template = Template::new()
///     .from_image("node:20")
///     .set_start_cmd("npm start", wait_for_timeout(5_000))
///     .skip_cache();
/// ```
///
/// # Serialisation
///
/// The `pub(crate)` helper `instruction_steps_from` converts accumulated
/// instructions to the wire type; `serialize` builds the full
/// `TemplateBuildStartV2` request body.  Both are called internally by the
/// HTTP build layer.
#[derive(Clone, Default)]
pub struct Template {
    /// Base Docker image (e.g. `"node:20"`). Set by [`Template::from_image`].
    pub(crate) base_image: Option<String>,
    /// Base E2B template ID or alias. Set by [`Template::from_template`].
    pub(crate) base_template: Option<String>,
    /// Optional private registry credentials for pulling the base image.
    pub(crate) registry_config: Option<RegistryConfig>,
    /// Ordered list of build instructions accumulated by the builder methods.
    pub(crate) instructions: Vec<Instruction>,
    /// Command to start the sandbox after the build completes.
    pub(crate) start_cmd: Option<String>,
    /// Ready-check command that signals the sandbox is accepting traffic.
    pub(crate) ready_cmd: Option<String>,
    /// When `true`, bypass the build cache for the entire template
    /// (corresponds to [`Template::skip_cache`]).
    pub(crate) force: bool,
    /// CPU count override for the build sandbox.
    pub(crate) cpu_count: Option<u32>,
    /// Memory override for the build sandbox, in MiB.
    pub(crate) memory_mb: Option<u32>,
}

impl Template {
    /// Create a new, empty template builder.
    ///
    /// Equivalent to `Template::default()`. Use the builder methods to
    /// configure the template before calling `Template::serialize`.
    pub fn new() -> Self {
        Self::default()
    }

    // ── Base-image / base-template entry points ───────────────────────────────

    /// Use a specific Docker image as the base for this template.
    ///
    /// Clears any previously set `base_template`.
    ///
    /// # Example
    ///
    /// ```rust
    /// use e2b_rs::template::Template;
    /// let t = Template::new().from_image("node:20");
    /// ```
    pub fn from_image(mut self, base_image: &str) -> Self {
        self.base_image = Some(base_image.to_string());
        self.base_template = None;
        self
    }

    /// Use the default E2B base image (`"e2bdev/base"`) as the base for this
    /// template.
    ///
    /// Equivalent to `from_image("e2bdev/base")`.
    pub fn from_base_image(self) -> Self {
        self.from_image("e2bdev/base")
    }

    /// Use an existing E2B template as the base for this template.
    ///
    /// `id_or_alias` may be either a numeric template ID or a human-readable
    /// alias. Clears any previously set `base_image`.
    pub fn from_template(mut self, id_or_alias: &str) -> Self {
        self.base_template = Some(id_or_alias.to_string());
        self.base_image = None;
        self
    }

    // ── Distro / runtime convenience variants ─────────────────────────────────

    /// Use a Debian image as the base for this template.
    ///
    /// Equivalent to `from_image(&format!("debian:{variant}"))`. The JS SDK
    /// defaults to `"stable"`; pass that string explicitly when you want the
    /// same default.
    ///
    /// # Example
    ///
    /// ```rust
    /// use e2b_rs::template::Template;
    /// let t = Template::new().from_debian_image("stable");
    /// ```
    pub fn from_debian_image(self, variant: &str) -> Self {
        self.from_image(&format!("debian:{variant}"))
    }

    /// Use an Ubuntu image as the base for this template.
    ///
    /// Equivalent to `from_image(&format!("ubuntu:{variant}"))`. The JS SDK
    /// defaults to `"latest"`; pass that string explicitly when you want the
    /// same default.
    ///
    /// # Example
    ///
    /// ```rust
    /// use e2b_rs::template::Template;
    /// let t = Template::new().from_ubuntu_image("latest");
    /// ```
    pub fn from_ubuntu_image(self, variant: &str) -> Self {
        self.from_image(&format!("ubuntu:{variant}"))
    }

    /// Use a Python image as the base for this template.
    ///
    /// Equivalent to `from_image(&format!("python:{version}"))`. The JS SDK
    /// defaults to `"3"`; pass that string explicitly when you want the same
    /// default.
    ///
    /// # Example
    ///
    /// ```rust
    /// use e2b_rs::template::Template;
    /// let t = Template::new().from_python_image("3.12");
    /// ```
    pub fn from_python_image(self, version: &str) -> Self {
        self.from_image(&format!("python:{version}"))
    }

    /// Use a Node.js image as the base for this template.
    ///
    /// Equivalent to `from_image(&format!("node:{variant}"))`. The JS SDK
    /// defaults to `"lts"`; pass that string explicitly when you want the same
    /// default.
    ///
    /// # Example
    ///
    /// ```rust
    /// use e2b_rs::template::Template;
    /// let t = Template::new().from_node_image("lts");
    /// ```
    pub fn from_node_image(self, variant: &str) -> Self {
        self.from_image(&format!("node:{variant}"))
    }

    /// Use a Bun image as the base for this template.
    ///
    /// Equivalent to `from_image(&format!("oven/bun:{variant}"))`. The JS SDK
    /// defaults to `"latest"`; pass that string explicitly when you want the
    /// same default.
    ///
    /// # Example
    ///
    /// ```rust
    /// use e2b_rs::template::Template;
    /// let t = Template::new().from_bun_image("latest");
    /// ```
    pub fn from_bun_image(self, variant: &str) -> Self {
        self.from_image(&format!("oven/bun:{variant}"))
    }

    // ── Private-registry entry points ─────────────────────────────────────────

    /// Use an image from Amazon ECR as the base for this template.
    ///
    /// Sets `base_image` to `image`, clears any previously set `base_template`,
    /// and stores the provided AWS credentials as [`RegistryConfig::Aws`].
    ///
    /// The credentials are used by the E2B build backend to authenticate the
    /// `docker pull` call; they are never stored in plaintext in logs (see
    /// [`RegistryConfig`]'s `Debug` implementation).
    ///
    /// # Example
    ///
    /// ```rust
    /// use e2b_rs::template::Template;
    /// let t = Template::new().from_aws_registry(
    ///     "123456789.dkr.ecr.us-east-1.amazonaws.com/my-image:latest",
    ///     "AKIAIOSFODNN7EXAMPLE",
    ///     "wJalrXUtnFEMI/K7MDENG/bPxRfiCYEXAMPLEKEY",
    ///     "us-east-1",
    /// );
    /// ```
    pub fn from_aws_registry(
        mut self,
        image: &str,
        access_key_id: &str,
        secret_access_key: &str,
        region: &str,
    ) -> Self {
        self.base_image = Some(image.to_string());
        self.base_template = None;
        self.registry_config = Some(RegistryConfig::Aws {
            access_key_id: access_key_id.to_string(),
            secret_access_key: secret_access_key.to_string(),
            region: region.to_string(),
        });
        self
    }

    /// Use an image from Google Container Registry or Artifact Registry as the
    /// base for this template.
    ///
    /// Sets `base_image` to `image`, clears any previously set `base_template`,
    /// and stores the provided GCP service-account JSON as
    /// [`RegistryConfig::Gcp`].
    ///
    /// `service_account_json` must be the raw JSON content of a GCP
    /// service-account key file (not a file path). The credentials are never
    /// logged in plaintext (see [`RegistryConfig`]'s `Debug` implementation).
    ///
    /// # Example
    ///
    /// ```rust
    /// use e2b_rs::template::Template;
    /// let sa_json = r#"{"type":"service_account","project_id":"my-proj"}"#;
    /// let t = Template::new()
    ///     .from_gcp_registry("gcr.io/my-proj/my-image:latest", sa_json);
    /// ```
    pub fn from_gcp_registry(mut self, image: &str, service_account_json: &str) -> Self {
        self.base_image = Some(image.to_string());
        self.base_template = None;
        self.registry_config = Some(RegistryConfig::Gcp {
            service_account_json: service_account_json.to_string(),
        });
        self
    }

    /// Parse a Dockerfile and configure this template from it.
    ///
    /// The content string must be the raw Dockerfile text (not a path). The
    /// parser is a minimal port of the JavaScript SDK's `dockerfileParser.ts`;
    /// only the subset of Dockerfile keywords that E2B supports are handled —
    /// `FROM`, `RUN`, `COPY`, `ADD`, `WORKDIR`, `USER`, `ENV`, `ARG`, `CMD`,
    /// `ENTRYPOINT`. Unknown keywords are silently ignored.
    ///
    /// Multi-stage Dockerfiles are **not** supported and return an error.
    ///
    /// # Errors
    ///
    /// Returns [`crate::Error::Template`] when:
    /// - The Dockerfile contains more than one `FROM` instruction.
    /// - The Dockerfile contains no `FROM` instruction.
    ///
    /// # Example
    ///
    /// ```rust
    /// use e2b_rs::template::Template;
    ///
    /// # fn main() -> e2b_rs::Result<()> {
    /// let t = Template::new()
    ///     .from_dockerfile("FROM node:20\nRUN npm install\n")?;
    /// # Ok(())
    /// # }
    /// ```
    pub fn from_dockerfile(mut self, content: &str) -> Result<Self> {
        let result = crate::template::dockerfile::parse_dockerfile(content)?;
        self.base_image = Some(result.base_image);
        self.base_template = None;

        for action in result.actions {
            self = apply_action(self, action);
        }

        Ok(self)
    }

    // ── Start-command / ready-command setters ─────────────────────────────────

    /// Set the sandbox start command and the corresponding ready-check.
    ///
    /// The `ready` argument is any [`ReadyCmd`] produced by the free functions
    /// in [`crate::template`] (e.g. [`crate::wait_for_port`],
    /// [`crate::wait_for_timeout`]).
    ///
    /// # Example
    ///
    /// ```rust
    /// use e2b_rs::template::Template;
    /// use e2b_rs::wait_for_port;
    ///
    /// let t = Template::new()
    ///     .from_base_image()
    ///     .set_start_cmd("node server.js", wait_for_port(3000));
    /// ```
    pub fn set_start_cmd(mut self, cmd: &str, ready: ReadyCmd) -> Self {
        self.start_cmd = Some(cmd.to_string());
        self.ready_cmd = Some(ready.cmd().to_string());
        self
    }

    /// Override only the ready-check command without changing the start
    /// command.
    pub fn set_ready_cmd(mut self, ready: ReadyCmd) -> Self {
        self.ready_cmd = Some(ready.cmd().to_string());
        self
    }

    /// Bypass the build cache for the entire template.
    ///
    /// Sets the `force` flag on the build request, causing all steps to be
    /// re-executed even if the cache would otherwise mark them as up-to-date.
    pub fn skip_cache(mut self) -> Self {
        self.force = true;
        self
    }

    // ── Internal serialisation helpers ────────────────────────────────────────

    /// Map a slice of [`Instruction`]s (typically hash-enriched) to
    /// [`crate::api::schema::TemplateStep`] wire types.
    ///
    /// The build layer calls this with the hash-filled instructions returned by
    /// [`crate::template::build_api::instructions_with_hashes`].
    pub(crate) fn instruction_steps_from(
        instructions: &[Instruction],
    ) -> Vec<crate::api::schema::TemplateStep> {
        instructions
            .iter()
            .map(|instr| crate::api::schema::TemplateStep {
                type_: instruction_type_str(instr.instruction_type),
                args: instr.args.clone(),
                files_hash: instr.files_hash.clone(),
                force: instr.force,
            })
            .collect()
    }

    /// Serialize the template into the wire-format
    /// [`crate::api::schema::TemplateBuildStartV2`] body.
    ///
    /// `steps` should be the result of [`Template::instruction_steps`] (or an
    /// enriched version with `files_hash` populated by the upload layer).
    ///
    /// This is a port of `serialize` in the JavaScript SDK (`index.ts:1301`).
    pub(crate) fn serialize(
        &self,
        steps: Vec<crate::api::schema::TemplateStep>,
    ) -> crate::api::schema::TemplateBuildStartV2 {
        crate::api::schema::TemplateBuildStartV2 {
            start_cmd: self.start_cmd.clone(),
            ready_cmd: self.ready_cmd.clone(),
            steps,
            force: self.force,
            from_image: self.base_image.clone(),
            from_template: self.base_template.clone(),
            from_image_registry: self.registry_config.as_ref().map(|r| r.to_wire()),
        }
    }

    /// Build this template and return a [`crate::template::handle::BuildHandle`]
    /// for streaming log entries.
    ///
    /// Consumes `self`. The returned handle allows callers to stream log entries
    /// via [`crate::template::handle::BuildHandle::next`] and to await build
    /// completion via [`crate::template::handle::BuildHandle::wait`].
    ///
    /// # Build context
    ///
    /// Files are uploaded from the **current working directory**
    /// (`std::env::current_dir()`). A configurable context path is a future
    /// carry-forward.
    ///
    /// # Errors
    ///
    /// Returns an error if the API key is missing or invalid, if any HTTP call
    /// fails, or if the build context cannot be read.
    pub async fn build(
        self,
        name: &str,
        opts: BuildOptions,
    ) -> crate::errors::Result<crate::template::handle::BuildHandle> {
        use crate::template::handle::{BuildHandle, wait_for_build_finish};
        use tokio::sync::{mpsc, oneshot};

        let (api, info) = self.setup_build(name, &opts).await?;

        // Spawn poll task; wire channels; return handle.
        let (tx_logs, rx_logs) = mpsc::channel::<crate::template::log::LogEntry>(128);
        let (tx_result, rx_result) =
            oneshot::channel::<crate::errors::Result<crate::template::types::BuildInfo>>();
        let info_clone = info.clone();
        let api_arc = std::sync::Arc::clone(&api);
        let tid = info.template_id.clone();
        let bid = info.build_id.clone();
        let task = tokio::spawn(async move {
            let r = wait_for_build_finish(api_arc, tid, bid, 200, tx_logs)
                .await
                .map(|()| info_clone);
            let _ = tx_result.send(r);
        });

        Ok(BuildHandle::new(rx_logs, rx_result, task, info))
    }

    /// Build this template in the background and return immediately with
    /// [`crate::template::types::BuildInfo`] containing the template and build
    /// identifiers.
    ///
    /// Unlike [`Template::build`], this method does **not** wait for the build
    /// to complete or stream log entries. The build continues asynchronously on
    /// E2B infrastructure.
    ///
    /// # Build context
    ///
    /// Files are uploaded from the **current working directory**. See
    /// [`Template::build`] for details.
    ///
    /// # Errors
    ///
    /// Returns an error if the API key is missing or invalid, if any HTTP call
    /// fails, or if the build context cannot be read.
    pub async fn build_in_background(
        self,
        name: &str,
        opts: BuildOptions,
    ) -> crate::errors::Result<crate::template::types::BuildInfo> {
        let (_api, info) = self.setup_build(name, &opts).await?;
        Ok(info)
    }

    // ── File-op builder methods ───────────────────────────────────────────────

    /// Copy a single source file or directory into the template image.
    ///
    /// `src` must be a relative path within the build context directory.
    /// Absolute paths and paths that escape the context via `..` return
    /// [`crate::errors::Error::InvalidArgument`].
    ///
    /// The resulting `COPY` instruction is appended with args
    /// `[src, dest, user, mode_octal]`, matching the shape consumed by the
    /// upload layer in Plan 5c.
    ///
    /// # Errors
    ///
    /// Returns [`crate::Error::InvalidArgument`] when `src` is absolute or
    /// escapes the context directory.
    ///
    /// # Example
    ///
    /// ```rust
    /// use e2b_rs::template::{Template, CopyOpts};
    ///
    /// # fn main() -> e2b_rs::Result<()> {
    /// let t = Template::new()
    ///     .copy("app.js", "/app/app.js", CopyOpts { user: Some("appuser".to_string()), ..Default::default() })?;
    /// # Ok(())
    /// # }
    /// ```
    pub fn copy(mut self, src: &str, dest: &str, opts: CopyOpts) -> crate::errors::Result<Self> {
        crate::template::files::validate_relative_path(src)?;
        self.instructions.push(crate::template::types::Instruction {
            instruction_type: crate::template::types::InstructionType::Copy,
            args: vec![
                src.to_string(),
                dest.to_string(),
                opts.user.clone().unwrap_or_default(),
                opts.mode.map(pad_octal).unwrap_or_default(),
            ],
            force: opts.force_upload.unwrap_or(false) || self.force,
            force_upload: opts.force_upload,
            files_hash: None,
            resolve_symlinks: opts.resolve_symlinks.unwrap_or(false),
        });
        Ok(self)
    }

    /// Copy multiple source items into the template image in a single call.
    ///
    /// Iterates over `items` and, for each [`crate::template::types::CopyItem`],
    /// iterates over its `src` paths and validates + pushes a `COPY`
    /// instruction for each. Validation rules are the same as [`Template::copy`].
    ///
    /// # Errors
    ///
    /// Returns [`crate::Error::InvalidArgument`] when any source path is
    /// absolute or escapes the context directory. Processing stops at the
    /// first invalid path.
    ///
    /// # Example
    ///
    /// ```rust
    /// use e2b_rs::template::Template;
    /// use e2b_rs::CopyItem;
    ///
    /// # fn main() -> e2b_rs::Result<()> {
    /// let t = Template::new().copy_items(vec![
    ///     CopyItem { src: vec!["src/main.rs".to_string()], dest: "/app/main.rs".to_string(), ..Default::default() },
    /// ])?;
    /// # Ok(())
    /// # }
    /// ```
    pub fn copy_items(
        mut self,
        items: Vec<crate::template::types::CopyItem>,
    ) -> crate::errors::Result<Self> {
        for item in items {
            for src in &item.src {
                crate::template::files::validate_relative_path(src)?;
                self.instructions.push(crate::template::types::Instruction {
                    instruction_type: crate::template::types::InstructionType::Copy,
                    args: vec![
                        src.clone(),
                        item.dest.clone(),
                        item.user.clone().unwrap_or_default(),
                        item.mode.map(pad_octal).unwrap_or_default(),
                    ],
                    force: item.force_upload.unwrap_or(false) || self.force,
                    force_upload: item.force_upload,
                    files_hash: None,
                    resolve_symlinks: item.resolve_symlinks,
                });
            }
        }
        Ok(self)
    }

    /// Remove files or directories inside the template image.
    ///
    /// Builds `rm [-r] [-f] <quoted-paths>` and pushes it as a `RUN`
    /// instruction. Flags are added in the order `-r` then `-f` to match the
    /// JavaScript SDK's `remove` implementation.
    ///
    /// # Example
    ///
    /// ```rust
    /// use e2b_rs::template::{Template, RemoveOpts};
    ///
    /// let t = Template::new()
    ///     .remove(&["/tmp/cache"], RemoveOpts { recursive: true, ..Default::default() });
    /// ```
    pub fn remove(mut self, paths: &[&str], opts: RemoveOpts) -> Self {
        let mut parts = vec!["rm".to_string()];
        if opts.recursive {
            parts.push("-r".to_string());
        }
        if opts.force {
            parts.push("-f".to_string());
        }
        for p in paths {
            parts.push(crate::utils::shell_quote(p));
        }
        let cmd = parts.join(" ");
        self.push_run(cmd, opts.user);
        self
    }

    /// Rename or move a file inside the template image.
    ///
    /// Builds `mv <quoted-src> <quoted-dest> [-f]` and pushes it as a `RUN`
    /// instruction. Mirrors the JavaScript SDK's `rename` (line 659).
    ///
    /// # Example
    ///
    /// ```rust
    /// use e2b_rs::template::{Template, RenameOpts};
    ///
    /// let t = Template::new()
    ///     .rename("old.txt", "new.txt", RenameOpts::default());
    /// ```
    pub fn rename(mut self, src: &str, dest: &str, opts: RenameOpts) -> Self {
        let mut parts = vec![
            "mv".to_string(),
            crate::utils::shell_quote(src),
            crate::utils::shell_quote(dest),
        ];
        if opts.force {
            parts.push("-f".to_string());
        }
        let cmd = parts.join(" ");
        self.push_run(cmd, opts.user);
        self
    }

    /// Create one or more directories inside the template image.
    ///
    /// Builds `mkdir -p [-m <mode>] <quoted-paths>` and pushes it as a `RUN`
    /// instruction. Mirrors the JavaScript SDK's `makeDir` (line 673).
    ///
    /// # Example
    ///
    /// ```rust
    /// use e2b_rs::template::{Template, MakeDirOpts};
    ///
    /// let t = Template::new()
    ///     .make_dir(&["/app/logs"], MakeDirOpts { mode: Some(0o755), ..Default::default() });
    /// ```
    pub fn make_dir(mut self, paths: &[&str], opts: MakeDirOpts) -> Self {
        let mut parts = vec!["mkdir".to_string(), "-p".to_string()];
        if let Some(mode) = opts.mode {
            parts.push(format!("-m {}", pad_octal(mode)));
        }
        for p in paths {
            parts.push(crate::utils::shell_quote(p));
        }
        let cmd = parts.join(" ");
        self.push_run(cmd, opts.user);
        self
    }

    /// Create a symbolic link inside the template image.
    ///
    /// Builds `ln -s [-f] <quoted-src> <quoted-dest>` and pushes it as a
    /// `RUN` instruction. Mirrors the JavaScript SDK's `makeSymlink`
    /// (line 688).
    ///
    /// # Example
    ///
    /// ```rust
    /// use e2b_rs::template::{Template, MakeSymlinkOpts};
    ///
    /// let t = Template::new()
    ///     .make_symlink("/usr/local/bin/node", "/usr/bin/node", MakeSymlinkOpts::default());
    /// ```
    pub fn make_symlink(mut self, src: &str, dest: &str, opts: MakeSymlinkOpts) -> Self {
        let mut parts = vec!["ln".to_string(), "-s".to_string()];
        if opts.force {
            parts.push("-f".to_string());
        }
        parts.push(crate::utils::shell_quote(src));
        parts.push(crate::utils::shell_quote(dest));
        let cmd = parts.join(" ");
        self.push_run(cmd, opts.user);
        self
    }

    // ── Command / env / package-installer / git-clone builder methods ─────────

    /// Run a single shell command inside the build sandbox.
    ///
    /// Appends a `RUN <command>` instruction. Use [`Template::run_cmds`] to
    /// join multiple commands with `&&` in a single layer.
    ///
    /// # Example
    ///
    /// ```rust
    /// use e2b_rs::template::{Template, RunCmdOpts};
    ///
    /// let t = Template::new()
    ///     .run_cmd("npm install", RunCmdOpts::default());
    /// ```
    pub fn run_cmd(mut self, command: &str, opts: RunCmdOpts) -> Self {
        self.push_run(command.to_string(), opts.user);
        self
    }

    /// Run multiple shell commands inside the build sandbox, joined with `&&`.
    ///
    /// Equivalent to `run_cmd(commands.join(" && "), opts)`. All commands run
    /// in a single image layer.
    ///
    /// # Example
    ///
    /// ```rust
    /// use e2b_rs::template::{Template, RunCmdOpts};
    ///
    /// let t = Template::new()
    ///     .run_cmds(&["apt-get update", "apt-get install -y curl"], RunCmdOpts::default());
    /// ```
    pub fn run_cmds(mut self, commands: &[&str], opts: RunCmdOpts) -> Self {
        self.push_run(commands.join(" && "), opts.user);
        self
    }

    /// Set the working directory for subsequent build instructions.
    ///
    /// Appends a `WORKDIR <path>` instruction. Port of `setWorkdir` in the JS
    /// SDK (line 728).
    ///
    /// # Example
    ///
    /// ```rust
    /// use e2b_rs::template::Template;
    ///
    /// let t = Template::new().set_workdir("/app");
    /// ```
    pub fn set_workdir(mut self, path: &str) -> Self {
        self.instructions.push(crate::template::types::Instruction {
            instruction_type: crate::template::types::InstructionType::Workdir,
            args: vec![path.to_string()],
            force: self.force,
            force_upload: None,
            files_hash: None,
            resolve_symlinks: false,
        });
        self
    }

    /// Set the user for subsequent build instructions.
    ///
    /// Appends a `USER <user>` instruction. Port of `setUser` in the JS SDK
    /// (line 739).
    ///
    /// # Example
    ///
    /// ```rust
    /// use e2b_rs::template::Template;
    ///
    /// let t = Template::new().set_user("appuser");
    /// ```
    pub fn set_user(mut self, user: &str) -> Self {
        self.instructions.push(crate::template::types::Instruction {
            instruction_type: crate::template::types::InstructionType::User,
            args: vec![user.to_string()],
            force: self.force,
            force_upload: None,
            files_hash: None,
            resolve_symlinks: false,
        });
        self
    }

    /// Set one or more environment variables in the template image.
    ///
    /// Appends an `ENV k1 v1 k2 v2 …` instruction with keys and values
    /// interleaved in [`std::collections::BTreeMap`] iteration order (sorted
    /// ascending by key). If `envs` is empty the method returns `self`
    /// unchanged. Port of `setEnvs` in the JS SDK (line 915).
    ///
    /// # Example
    ///
    /// ```rust
    /// use std::collections::BTreeMap;
    /// use e2b_rs::template::Template;
    ///
    /// let mut envs = BTreeMap::new();
    /// envs.insert("PORT".to_string(), "3000".to_string());
    /// envs.insert("NODE_ENV".to_string(), "production".to_string());
    /// let t = Template::new().set_envs(envs);
    /// ```
    pub fn set_envs(mut self, envs: std::collections::BTreeMap<String, String>) -> Self {
        if envs.is_empty() {
            return self;
        }
        let args: Vec<String> = envs.into_iter().flat_map(|(k, v)| [k, v]).collect();
        self.instructions.push(crate::template::types::Instruction {
            instruction_type: crate::template::types::InstructionType::Env,
            args,
            force: self.force,
            force_upload: None,
            files_hash: None,
            resolve_symlinks: false,
        });
        self
    }

    /// Install Python packages with `pip install`.
    ///
    /// When `packages` is empty, installs from the current directory (`.`).
    /// When `opts.global` is `true` (the default), runs as root and installs
    /// globally; when `false`, passes `--user` and runs without a user
    /// override. Port of `pipInstall` in the JS SDK (line 750).
    ///
    /// # Example
    ///
    /// ```rust
    /// use e2b_rs::template::{Template, PipInstallOpts};
    ///
    /// // Global install (default):
    /// let t = Template::new().pip_install(&["requests"], PipInstallOpts::default());
    ///
    /// // User-local install:
    /// let t = Template::new()
    ///     .pip_install(&["requests"], PipInstallOpts { global: false });
    /// ```
    pub fn pip_install(mut self, packages: &[&str], opts: PipInstallOpts) -> Self {
        let mut parts = vec!["pip".to_string(), "install".to_string()];
        if !opts.global {
            parts.push("--user".to_string());
        }
        if packages.is_empty() {
            parts.push(".".to_string());
        } else {
            for p in packages {
                parts.push(p.to_string());
            }
        }
        let cmd = parts.join(" ");
        let user = if opts.global {
            Some("root".to_string())
        } else {
            None
        };
        self.push_run(cmd, user);
        self
    }

    /// Install Node.js packages with `npm install`.
    ///
    /// Passes `-g` when `opts.global` is `true` (also runs as root) and
    /// `--save-dev` when `opts.dev` is `true`. Port of `npmInstall` in the JS
    /// SDK (line 778).
    ///
    /// # Example
    ///
    /// ```rust
    /// use e2b_rs::template::{Template, NpmInstallOpts};
    ///
    /// let t = Template::new()
    ///     .npm_install(&["typescript"], NpmInstallOpts { global: true, dev: false });
    /// ```
    pub fn npm_install(mut self, packages: &[&str], opts: NpmInstallOpts) -> Self {
        let mut parts = vec!["npm".to_string(), "install".to_string()];
        if opts.global {
            parts.push("-g".to_string());
        }
        if opts.dev {
            parts.push("--save-dev".to_string());
        }
        for p in packages {
            parts.push(p.to_string());
        }
        let cmd = parts.join(" ");
        let user = if opts.global {
            Some("root".to_string())
        } else {
            None
        };
        self.push_run(cmd, user);
        self
    }

    /// Install packages with `bun install`.
    ///
    /// Passes `-g` when `opts.global` is `true` (also runs as root via
    /// `user: "root"`) and `--dev` when `opts.dev` is `true`. Port of
    /// `bunInstall` in the JS SDK (line 805).
    ///
    /// # Example
    ///
    /// ```rust
    /// use e2b_rs::template::{Template, BunInstallOpts};
    ///
    /// let t = Template::new()
    ///     .bun_install(&["elysia"], BunInstallOpts { global: false, dev: false });
    /// ```
    pub fn bun_install(mut self, packages: &[&str], opts: BunInstallOpts) -> Self {
        let mut parts = vec!["bun".to_string(), "install".to_string()];
        if opts.global {
            parts.push("-g".to_string());
        }
        if opts.dev {
            parts.push("--dev".to_string());
        }
        for p in packages {
            parts.push(p.to_string());
        }
        let cmd = parts.join(" ");
        let user = if opts.global {
            Some("root".to_string())
        } else {
            None
        };
        self.push_run(cmd, user);
        self
    }

    /// Install system packages with `apt-get`.
    ///
    /// Runs as root and appends a two-command `RUN` instruction:
    /// `apt-get update && DEBIAN_FRONTEND=noninteractive DEBCONF_NOWARNINGS=yes
    /// apt-get install -y [--no-install-recommends] [--fix-missing] <packages>`.
    /// Port of `aptInstall` in the JS SDK (line 832).
    ///
    /// # Example
    ///
    /// ```rust
    /// use e2b_rs::template::{Template, AptInstallOpts};
    ///
    /// let t = Template::new().apt_install(
    ///     &["curl", "git"],
    ///     AptInstallOpts { no_install_recommends: true, ..Default::default() },
    /// );
    /// ```
    pub fn apt_install(mut self, packages: &[&str], opts: AptInstallOpts) -> Self {
        let install_cmd = format!(
            "DEBIAN_FRONTEND=noninteractive DEBCONF_NOWARNINGS=yes apt-get install -y {}{}{}",
            if opts.no_install_recommends {
                "--no-install-recommends "
            } else {
                ""
            },
            if opts.fix_missing {
                "--fix-missing "
            } else {
                ""
            },
            packages.join(" ")
        );
        let cmd = format!("apt-get update && {install_cmd}");
        self.push_run(cmd, Some("root".to_string()));
        self
    }

    /// Clone a Git repository into the template image.
    ///
    /// Builds a `git clone <url> [--branch <branch> --single-branch]
    /// [--depth <depth>] [<path>]` command and appends it as a `RUN`
    /// instruction. The `url`, `branch`, and `path` arguments are
    /// shell-quoted (empty becomes `''`; strings with special characters are
    /// single-quoted). Port of `gitClone` in the JS SDK (line 866).
    ///
    /// # Example
    ///
    /// ```rust
    /// use e2b_rs::template::{Template, GitCloneOpts};
    ///
    /// let t = Template::new().git_clone(
    ///     "https://github.com/owner/repo.git",
    ///     Some("/opt/repo"),
    ///     GitCloneOpts { branch: Some("main".to_string()), depth: Some(1), user: None },
    /// );
    /// ```
    pub fn git_clone(mut self, url: &str, path: Option<&str>, opts: GitCloneOpts) -> Self {
        let mut parts = vec![
            "git".to_string(),
            "clone".to_string(),
            crate::utils::shell_quote(url),
        ];
        if let Some(branch) = &opts.branch {
            parts.push(format!("--branch {}", crate::utils::shell_quote(branch)));
            parts.push("--single-branch".to_string());
        }
        if let Some(depth) = opts.depth {
            parts.push(format!("--depth {depth}"));
        }
        if let Some(p) = path {
            parts.push(crate::utils::shell_quote(p));
        }
        let cmd = parts.join(" ");
        self.push_run(cmd, opts.user);
        self
    }

    // ── Private run-instruction helper ────────────────────────────────────────

    /// Push a `RUN` instruction.
    ///
    /// `cmd` becomes `args[0]`; `user`, if present, becomes `args[1]`.
    /// The instruction's `force` flag is inherited from `self.force` (i.e.
    /// the template-level skip-cache setting).
    fn push_run(&mut self, cmd: String, user: Option<String>) {
        let mut args = vec![cmd];
        if let Some(u) = user {
            args.push(u);
        }
        self.instructions.push(crate::template::types::Instruction {
            instruction_type: crate::template::types::InstructionType::Run,
            args,
            force: self.force,
            force_upload: None,
            files_hash: None,
            resolve_symlinks: false,
        });
    }

    /// Shared setup for [`Template::build`] and [`Template::build_in_background`].
    ///
    /// Performs steps 1–8 of the build pipeline:
    /// 1. Resolve [`crate::ConnectionConfig`] and construct a shared
    ///    [`crate::api::client::ApiClient`] (wrapped in an [`std::sync::Arc`]).
    /// 2. Apply cpu/memory defaults (`2` vCPUs / `1024` MiB).
    /// 3. Request a build slot via `POST /v3/templates`, passing the whole
    ///    `name` unchanged (no colon splitting).
    /// 4. Resolve the build context directory (`std::env::current_dir()`).
    /// 5. Populate `files_hash` on `COPY` instructions.
    /// 6. Upload file-context archives for uncached instructions.
    /// 7. Trigger the build (`POST /v2/templates/{id}/builds/{bid}`).
    /// 8. Construct [`crate::template::types::BuildInfo`] with
    ///    `name = alias = name` (whole input) and `tags` from the API response.
    ///
    /// Returns `(api_client, build_info)`.  The caller either spawns a poll
    /// task on top (`build`) or returns `build_info` immediately
    /// (`build_in_background`).
    async fn setup_build(
        self,
        name: &str,
        opts: &BuildOptions,
    ) -> crate::errors::Result<(
        std::sync::Arc<crate::api::client::ApiClient>,
        crate::template::types::BuildInfo,
    )> {
        use std::sync::Arc;

        use crate::api::client::ApiClient;
        use crate::connection_config::{ConnectionConfig, ConnectionConfigOpts};
        use crate::template::build_api::{
            instructions_with_hashes, request_build, trigger_build, upload_build_context,
        };

        // 1. Resolve ConnectionConfig + ApiClient.
        let config = ConnectionConfig::new(ConnectionConfigOpts {
            api_key: opts.api_key.clone(),
            domain: opts.domain.clone(),
            api_url: opts.api_url.clone(),
            request_timeout_ms: opts.request_timeout_ms,
            ..Default::default()
        });
        let api = Arc::new(ApiClient::new(&config, true)?);

        // 2. Apply cpu/memory defaults — mirrors JS `cpuCount ?? 2, memoryMB ?? 1024`.
        let cpu = opts.cpu_count.or(self.cpu_count).unwrap_or(2);
        let mem = opts.memory_mb.or(self.memory_mb).unwrap_or(1024);

        // 3. Request a build slot — pass the WHOLE name; no colon splitting.
        let resp = request_build(&api, name, &[], Some(cpu), Some(mem)).await?;

        // 4. Resolve build context directory (default: cwd).
        let context = std::env::current_dir().map_err(|e| {
            crate::errors::Error::Internal(format!("failed to get current directory: {e}"))
        })?;

        // 5. Populate `files_hash` on COPY instructions.
        let instrs = instructions_with_hashes(&self.instructions, &context)?;

        // 6. Upload file-context archives for instructions not already cached.
        let http = reqwest::Client::new();
        upload_build_context(&api, &http, &resp.template_id, &instrs, &context).await?;

        // 7. Trigger the build; honour skip_cache override.
        let steps = Template::instruction_steps_from(&instrs);
        let mut body = self.serialize(steps);
        body.force = opts.skip_cache || body.force;
        trigger_build(&api, &resp.template_id, &resp.build_id, &body).await?;

        // 8. Construct BuildInfo — alias = name = whole input; tags from response.
        let info = crate::template::types::BuildInfo {
            template_id: resp.template_id,
            build_id: resp.build_id,
            name: Some(name.to_string()),
            alias: Some(name.to_string()),
            tags: resp.tags,
        };

        Ok((api, info))
    }
}

// ──────────────────────────────────────────────────────────────────────────────
// Private helpers
// ──────────────────────────────────────────────────────────────────────────────

/// Apply a single [`DockerfileAction`] to the template, returning the updated
/// template.
///
/// The mapping follows the JavaScript SDK's `parseDockerfile` → builder method
/// call pattern:
///
/// | [`DockerfileAction`] | Effect |
/// |---|---|
/// | `SetUser(u)` | push `USER u` instruction |
/// | `SetWorkdir(p)` | push `WORKDIR p` instruction |
/// | `Copy { src, dest, user }` | push `COPY src dest [user] [""]` instruction |
/// | `RunCmd(cmd)` | push `RUN cmd` instruction |
/// | `SetEnvs(map)` | push `ENV k1 v1 k2 v2 …` instruction |
/// | `SetStartCmd { cmd, ready }` | set `start_cmd` + `ready_cmd` |
fn apply_action(mut template: Template, action: DockerfileAction) -> Template {
    match action {
        DockerfileAction::SetUser(user) => {
            template.instructions.push(Instruction {
                instruction_type: InstructionType::User,
                args: vec![user],
                force: false,
                force_upload: None,
                files_hash: None,
                resolve_symlinks: false,
            });
        }
        DockerfileAction::SetWorkdir(path) => {
            template.instructions.push(Instruction {
                instruction_type: InstructionType::Workdir,
                args: vec![path],
                force: false,
                force_upload: None,
                files_hash: None,
                resolve_symlinks: false,
            });
        }
        DockerfileAction::Copy { src, dest, user } => {
            // Mirror the JS SDK copy() args format:
            // [src, dest, user || '', mode || '']
            template.instructions.push(Instruction {
                instruction_type: InstructionType::Copy,
                args: vec![src, dest, user.unwrap_or_default(), String::new()],
                force: false,
                force_upload: None,
                files_hash: None,
                resolve_symlinks: false,
            });
        }
        DockerfileAction::RunCmd(cmd) => {
            template.instructions.push(Instruction {
                instruction_type: InstructionType::Run,
                args: vec![cmd],
                force: false,
                force_upload: None,
                files_hash: None,
                resolve_symlinks: false,
            });
        }
        DockerfileAction::SetEnvs(map) => {
            if !map.is_empty() {
                // Mirror JS setEnvs: flatMap(([k, v]) => [k, v])
                let args: Vec<String> = map.into_iter().flat_map(|(k, v)| [k, v]).collect();
                template.instructions.push(Instruction {
                    instruction_type: InstructionType::Env,
                    args,
                    force: false,
                    force_upload: None,
                    files_hash: None,
                    resolve_symlinks: false,
                });
            }
        }
        DockerfileAction::SetStartCmd { cmd, ready } => {
            template.start_cmd = Some(cmd);
            template.ready_cmd = Some(ready.cmd().to_string());
        }
    }
    template
}

/// Convert an [`InstructionType`] to its wire-format string representation.
fn instruction_type_str(ty: InstructionType) -> String {
    match ty {
        InstructionType::Copy => "COPY".to_string(),
        InstructionType::Env => "ENV".to_string(),
        InstructionType::Run => "RUN".to_string(),
        InstructionType::Workdir => "WORKDIR".to_string(),
        InstructionType::User => "USER".to_string(),
    }
}

/// Format a Unix permission mode as a zero-padded four-digit octal string.
///
/// Port of the JavaScript SDK's `padOctal` (`utils.ts:352`):
/// `mode.toString(8).padStart(4, '0')`.
///
/// # Example
///
/// ```text
/// pad_octal(0o755) // "0755"
/// pad_octal(0o644) // "0644"
/// ```
fn pad_octal(mode: u32) -> String {
    format!("{mode:04o}")
}

// ──────────────────────────────────────────────────────────────────────────────
// Tests
// ──────────────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use super::*;
    use crate::api::schema::FromImageRegistry;
    use crate::wait_for_timeout;

    // ── from_dockerfile → base_image + instructions ───────────────────────────

    #[test]
    fn from_dockerfile_sets_base_and_instructions() {
        let dockerfile = "\
FROM node:20
RUN npm install
COPY . /app
WORKDIR /app
USER myuser
ENV FOO=bar
CMD npm start
";
        let template = Template::new()
            .from_dockerfile(dockerfile)
            .expect("valid Dockerfile");

        // base_image extracted from FROM
        assert_eq!(template.base_image.as_deref(), Some("node:20"));

        // start_cmd from CMD
        assert_eq!(template.start_cmd.as_deref(), Some("npm start"));
        // ready_cmd from CMD default (20s)
        assert_eq!(template.ready_cmd.as_deref(), Some("sleep 20"));

        // Check instruction types in order:
        // Docker defaults: USER root, WORKDIR /
        // RUN, COPY, WORKDIR, USER, ENV
        // No E2B defaults because USER + WORKDIR were set
        let types: Vec<InstructionType> = template
            .instructions
            .iter()
            .map(|i| i.instruction_type)
            .collect();

        assert_eq!(
            types,
            vec![
                InstructionType::User,    // root (docker default)
                InstructionType::Workdir, // / (docker default)
                InstructionType::Run,     // npm install
                InstructionType::Copy,    // . /app
                InstructionType::Workdir, // /app
                InstructionType::User,    // myuser
                InstructionType::Env,     // FOO=bar
            ]
        );

        // Verify COPY args: [src, dest, user_or_empty, mode_or_empty]
        let copy = template
            .instructions
            .iter()
            .find(|i| i.instruction_type == InstructionType::Copy)
            .expect("expected Copy instruction");
        assert_eq!(copy.args[0], ".");
        assert_eq!(copy.args[1], "/app");
        assert_eq!(copy.args[2], ""); // no --chown

        // Verify ENV args: interleaved [k, v]
        let env_instr = template
            .instructions
            .iter()
            .find(|i| i.instruction_type == InstructionType::Env)
            .expect("expected Env instruction");
        assert_eq!(env_instr.args, vec!["FOO", "bar"]);
    }

    // ── serialize → TemplateBuildStartV2 mapping ──────────────────────────────

    #[test]
    fn serialize_maps_from_image_and_steps() {
        let template = Template::new()
            .from_image("node:20")
            .set_start_cmd("npm start", wait_for_timeout(1_000))
            .skip_cache();

        let steps = Template::instruction_steps_from(&template.instructions);
        let body = template.serialize(steps);

        assert_eq!(body.from_image.as_deref(), Some("node:20"));
        assert_eq!(body.start_cmd.as_deref(), Some("npm start"));
        assert!(body.force, "skip_cache should set force=true");
        assert_eq!(body.from_template, None);
        assert!(body.from_image_registry.is_none());
        // No instructions were added directly, so steps should be empty
        assert!(body.steps.is_empty());
    }

    // ── RegistryConfig::to_wire — all three variants ─────────────────────────

    #[test]
    fn registry_to_wire_aws() {
        let cfg = RegistryConfig::Aws {
            access_key_id: "AKID".to_string(),
            secret_access_key: "SECRET".to_string(),
            region: "us-east-1".to_string(),
        };
        let wire = cfg.to_wire();
        match wire {
            FromImageRegistry::AwsRegistry(aws) => {
                assert_eq!(aws.aws_access_key_id, "AKID");
                assert_eq!(aws.aws_secret_access_key, "SECRET");
                assert_eq!(aws.aws_region, "us-east-1");
                assert_eq!(
                    aws.type_,
                    crate::api::schema::AwsRegistryType::Aws,
                    "type discriminator must be Aws"
                );
            }
            other => panic!("expected AwsRegistry, got {other:?}"),
        }
    }

    #[test]
    fn registry_to_wire_gcp() {
        let cfg = RegistryConfig::Gcp {
            service_account_json: r#"{"type":"service_account"}"#.to_string(),
        };
        let wire = cfg.to_wire();
        match wire {
            FromImageRegistry::GcpRegistry(gcp) => {
                assert_eq!(gcp.service_account_json, r#"{"type":"service_account"}"#);
                assert_eq!(
                    gcp.type_,
                    crate::api::schema::GcpRegistryType::Gcp,
                    "type discriminator must be Gcp"
                );
            }
            other => panic!("expected GcpRegistry, got {other:?}"),
        }
    }

    #[test]
    fn registry_to_wire_general() {
        let cfg = RegistryConfig::General {
            username: "user".to_string(),
            password: "pass".to_string(),
        };
        let wire = cfg.to_wire();
        match wire {
            FromImageRegistry::GeneralRegistry(general) => {
                assert_eq!(general.username, "user");
                assert_eq!(general.password, "pass");
                assert_eq!(
                    general.type_,
                    crate::api::schema::GeneralRegistryType::Registry,
                    "type discriminator must be Registry"
                );
            }
            other => panic!("expected GeneralRegistry, got {other:?}"),
        }
    }

    // ── Secret-redaction tests ────────────────────────────────────────────────

    #[test]
    fn registry_config_debug_redacts_secrets() {
        let aws = RegistryConfig::Aws {
            access_key_id: "MY_ACCESS_KEY_ID".to_string(),
            secret_access_key: "MY_SECRET_KEY".to_string(),
            region: "eu-west-1".to_string(),
        };
        let debug_str = format!("{aws:?}");
        assert!(
            !debug_str.contains("MY_ACCESS_KEY_ID"),
            "access_key_id must not appear in Debug: {debug_str}"
        );
        assert!(
            !debug_str.contains("MY_SECRET_KEY"),
            "secret_access_key must not appear in Debug: {debug_str}"
        );
        assert!(
            debug_str.contains("<redacted>"),
            "Debug must contain '<redacted>': {debug_str}"
        );
        // Non-secret fields should still appear
        assert!(
            debug_str.contains("eu-west-1"),
            "region should appear in Debug: {debug_str}"
        );

        let gcp = RegistryConfig::Gcp {
            service_account_json: "SUPER_SECRET_JSON".to_string(),
        };
        let gcp_debug = format!("{gcp:?}");
        assert!(
            !gcp_debug.contains("SUPER_SECRET_JSON"),
            "service_account_json must not appear in Debug: {gcp_debug}"
        );

        let general = RegistryConfig::General {
            username: "myuser".to_string(),
            password: "MY_PASSWORD".to_string(),
        };
        let gen_debug = format!("{general:?}");
        assert!(
            !gen_debug.contains("MY_PASSWORD"),
            "password must not appear in Debug: {gen_debug}"
        );
        // username is not a secret
        assert!(
            gen_debug.contains("myuser"),
            "username should appear in Debug: {gen_debug}"
        );
    }

    #[test]
    fn build_options_debug_redacts_api_key() {
        let opts = BuildOptions {
            api_key: Some("e2b_super_secret_key".to_string()),
            domain: Some("e2b.app".to_string()),
            ..Default::default()
        };
        let debug_str = format!("{opts:?}");
        assert!(
            !debug_str.contains("e2b_super_secret_key"),
            "api_key must not appear in Debug: {debug_str}"
        );
        assert!(
            debug_str.contains("<redacted>"),
            "Debug must contain '<redacted>': {debug_str}"
        );
        assert!(
            debug_str.contains("e2b.app"),
            "domain should appear in Debug: {debug_str}"
        );
    }

    // ── instruction_steps maps types to strings correctly ─────────────────────

    #[test]
    fn instruction_steps_type_strings() {
        let template = Template::new()
            .from_dockerfile("FROM scratch\nRUN echo hi\nCOPY . /app\n")
            .expect("valid Dockerfile");

        let steps = Template::instruction_steps_from(&template.instructions);
        // Collect type_ strings
        let type_strings: Vec<&str> = steps.iter().map(|s| s.type_.as_str()).collect();

        // We should have USER, WORKDIR (docker defaults), RUN, COPY,
        // USER, WORKDIR (e2b defaults)
        assert!(
            type_strings.contains(&"RUN"),
            "should contain RUN: {type_strings:?}"
        );
        assert!(
            type_strings.contains(&"COPY"),
            "should contain COPY: {type_strings:?}"
        );
        assert!(
            type_strings.contains(&"USER"),
            "should contain USER: {type_strings:?}"
        );
        assert!(
            type_strings.contains(&"WORKDIR"),
            "should contain WORKDIR: {type_strings:?}"
        );

        // All strings must be from the known set
        for s in &type_strings {
            assert!(
                ["COPY", "RUN", "ENV", "WORKDIR", "USER"].contains(s),
                "unexpected type string: {s}"
            );
        }
    }

    // ── builder entry-point smoke tests ──────────────────────────────────────

    #[test]
    fn from_base_image_uses_default() {
        let t = Template::new().from_base_image();
        assert_eq!(t.base_image.as_deref(), Some("e2bdev/base"));
    }

    #[test]
    fn from_template_clears_base_image() {
        let t = Template::new()
            .from_image("node:20")
            .from_template("base-sandbox");
        assert_eq!(t.base_template.as_deref(), Some("base-sandbox"));
        assert_eq!(t.base_image, None);
    }

    #[test]
    fn skip_cache_sets_force() {
        let t = Template::new().from_base_image().skip_cache();
        assert!(t.force);
    }

    #[test]
    fn serialize_includes_from_image_registry() {
        let t = Template::new().from_base_image();
        // Manually set registry_config to test the round-trip
        let mut t = t;
        t.registry_config = Some(RegistryConfig::General {
            username: "u".to_string(),
            password: "p".to_string(),
        });
        let steps = Template::instruction_steps_from(&t.instructions);
        let body = t.serialize(steps);
        assert!(body.from_image_registry.is_some());
    }

    // ── BTreeMap ordering: SetEnvs uses BTreeMap so args are sorted ──────────

    #[test]
    fn set_envs_interleaved_args_are_sorted() {
        // Build a map via BTreeMap directly (parse_dockerfile uses BTreeMap too)
        let mut map = BTreeMap::new();
        map.insert("Z".to_string(), "last".to_string());
        map.insert("A".to_string(), "first".to_string());

        let mut t = Template::new().from_base_image();
        let action = DockerfileAction::SetEnvs(map);
        t = apply_action(t, action);

        // Find the ENV instruction
        let env = t
            .instructions
            .iter()
            .find(|i| i.instruction_type == InstructionType::Env)
            .expect("expected Env instruction");

        // BTreeMap iteration is sorted ascending
        assert_eq!(env.args, vec!["A", "first", "Z", "last"]);
    }

    // ── build_in_background — no poll task ─────────────────────────────────────

    /// `build_in_background` must request a slot, trigger the build, and return
    /// the [`BuildInfo`] immediately WITHOUT polling the build-status endpoint.
    #[tokio::test]
    async fn build_in_background_skips_poll() {
        use wiremock::matchers::{method, path, path_regex};
        use wiremock::{Mock, MockServer, ResponseTemplate};

        let server = MockServer::start().await;

        // request_build → returns template/build ids.
        Mock::given(method("POST"))
            .and(path("/v3/templates"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "templateID": "tpl_bg",
                "buildID": "bld_bg",
                "aliases": [],
                "names": ["my-env"],
                "public": false,
                "tags": []
            })))
            .mount(&server)
            .await;

        // trigger_build → 204.
        Mock::given(method("POST"))
            .and(path("/v2/templates/tpl_bg/builds/bld_bg"))
            .respond_with(ResponseTemplate::new(204))
            .mount(&server)
            .await;

        // status endpoint must NEVER be called by build_in_background.
        Mock::given(method("GET"))
            .and(path_regex(r"^/templates/.+/builds/.+/status$"))
            .respond_with(ResponseTemplate::new(200))
            .expect(0)
            .mount(&server)
            .await;

        // A template with NO COPY instructions → no file uploads happen.
        let template = Template::new().from_image("node:20");
        let opts = BuildOptions {
            api_key: Some("e2b_0123456789abcdef".to_string()),
            api_url: Some(server.uri()),
            ..Default::default()
        };

        let info = template
            .build_in_background("my-env", opts)
            .await
            .expect("build_in_background should succeed");

        assert_eq!(info.template_id, "tpl_bg");
        assert_eq!(info.build_id, "bld_bg");
        assert_eq!(info.name.as_deref(), Some("my-env"));
        assert_eq!(info.alias.as_deref(), Some("my-env"));

        // wiremock asserts the status-endpoint `expect(0)` on server drop.
    }

    // ── tagged name sent whole (Fix 1) ────────────────────────────────────────

    /// A name in `"name:tag"` form must be forwarded verbatim to
    /// `POST /v3/templates` — the colon must NOT be stripped.
    /// The returned [`BuildInfo`] must have `name == Some("my-env:v1")` and
    /// `alias == Some("my-env:v1")`.
    #[tokio::test]
    async fn tagged_name_sent_whole_not_split() {
        use wiremock::matchers::{body_partial_json, method, path};
        use wiremock::{Mock, MockServer, ResponseTemplate};

        let server = MockServer::start().await;

        // The POST body must contain the WHOLE name including the colon.
        Mock::given(method("POST"))
            .and(path("/v3/templates"))
            .and(body_partial_json(
                serde_json::json!({ "name": "my-env:v1" }),
            ))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "templateID": "tpl_tag",
                "buildID": "bld_tag",
                "aliases": [],
                "names": ["my-env:v1"],
                "public": false,
                "tags": []
            })))
            .mount(&server)
            .await;

        Mock::given(method("POST"))
            .and(path("/v2/templates/tpl_tag/builds/bld_tag"))
            .respond_with(ResponseTemplate::new(204))
            .mount(&server)
            .await;

        let template = Template::new().from_image("node:20");
        let opts = BuildOptions {
            api_key: Some("e2b_0123456789abcdef".to_string()),
            api_url: Some(server.uri()),
            ..Default::default()
        };

        let info = template
            .build_in_background("my-env:v1", opts)
            .await
            .expect("build_in_background should succeed");

        assert_eq!(info.template_id, "tpl_tag");
        assert_eq!(info.name.as_deref(), Some("my-env:v1"));
        assert_eq!(info.alias.as_deref(), Some("my-env:v1"));
    }

    // ── from_*_image builder variants ────────────────────────────────────────

    #[test]
    fn from_python_image_sets_base() {
        let t = Template::new().from_python_image("3.12");
        assert_eq!(t.base_image.as_deref(), Some("python:3.12"));
        assert_eq!(t.base_template, None);
    }

    #[test]
    fn from_debian_image_sets_base() {
        let t = Template::new().from_debian_image("stable");
        assert_eq!(t.base_image.as_deref(), Some("debian:stable"));
        assert_eq!(t.base_template, None);
    }

    #[test]
    fn from_ubuntu_image_sets_base() {
        let t = Template::new().from_ubuntu_image("latest");
        assert_eq!(t.base_image.as_deref(), Some("ubuntu:latest"));
        assert_eq!(t.base_template, None);
    }

    #[test]
    fn from_node_image_sets_base() {
        let t = Template::new().from_node_image("lts");
        assert_eq!(t.base_image.as_deref(), Some("node:lts"));
        assert_eq!(t.base_template, None);
    }

    #[test]
    fn from_bun_image_sets_base() {
        let t = Template::new().from_bun_image("latest");
        assert_eq!(t.base_image.as_deref(), Some("oven/bun:latest"));
        assert_eq!(t.base_template, None);
    }

    #[test]
    fn from_image_variants_clear_base_template() {
        // Verify that each from_*_image clears a previously set base_template.
        let t = Template::new()
            .from_template("old-tpl")
            .from_python_image("3.11");
        assert_eq!(t.base_template, None);
        assert_eq!(t.base_image.as_deref(), Some("python:3.11"));
    }

    // ── from_aws_registry / from_gcp_registry ────────────────────────────────

    #[test]
    fn from_aws_registry_sets_config() {
        let t = Template::new().from_aws_registry(
            "123456789.dkr.ecr.us-east-1.amazonaws.com/my-image:latest",
            "AKID_TEST",
            "SECRET_TEST",
            "us-east-1",
        );
        assert_eq!(
            t.base_image.as_deref(),
            Some("123456789.dkr.ecr.us-east-1.amazonaws.com/my-image:latest")
        );
        assert_eq!(t.base_template, None);
        match t.registry_config {
            Some(RegistryConfig::Aws {
                access_key_id,
                secret_access_key,
                region,
            }) => {
                assert_eq!(access_key_id, "AKID_TEST");
                assert_eq!(secret_access_key, "SECRET_TEST");
                assert_eq!(region, "us-east-1");
            }
            other => panic!("expected RegistryConfig::Aws, got {other:?}"),
        }
    }

    #[test]
    fn from_gcp_registry_sets_config() {
        let sa_json = r#"{"type":"service_account","project_id":"my-proj"}"#;
        let t = Template::new().from_gcp_registry("gcr.io/my-proj/my-image:latest", sa_json);
        assert_eq!(
            t.base_image.as_deref(),
            Some("gcr.io/my-proj/my-image:latest")
        );
        assert_eq!(t.base_template, None);
        match t.registry_config {
            Some(RegistryConfig::Gcp {
                service_account_json,
            }) => {
                assert_eq!(service_account_json, sa_json);
            }
            other => panic!("expected RegistryConfig::Gcp, got {other:?}"),
        }
    }

    // ── File-op builder method tests ─────────────────────────────────────────

    #[test]
    fn copy_pushes_copy_instruction() {
        let t = Template::new()
            .copy(
                "app.js",
                "/app/app.js",
                CopyOpts {
                    user: Some("me".to_string()),
                    mode: Some(0o755),
                    force_upload: Some(true),
                    resolve_symlinks: Some(true),
                },
            )
            .expect("valid relative src");

        assert_eq!(t.instructions.len(), 1);
        let instr = &t.instructions[0];
        assert_eq!(
            instr.instruction_type,
            crate::template::types::InstructionType::Copy
        );
        // args: [src, dest, user, mode_octal]
        assert_eq!(instr.args, vec!["app.js", "/app/app.js", "me", "0755"]);
        // force_upload propagated
        assert_eq!(instr.force_upload, Some(true));
        // force: force_upload=true || self.force=false → true
        assert!(instr.force);
        // resolve_symlinks propagated
        assert!(instr.resolve_symlinks);
    }

    #[test]
    fn copy_pushes_default_user_and_mode_as_empty_strings() {
        let t = Template::new()
            .copy("src/lib.rs", "/app/lib.rs", CopyOpts::default())
            .expect("valid relative src");

        let instr = &t.instructions[0];
        // user and mode default to empty string
        assert_eq!(instr.args, vec!["src/lib.rs", "/app/lib.rs", "", ""]);
        assert_eq!(instr.force_upload, None);
        assert!(!instr.force);
        assert!(!instr.resolve_symlinks);
    }

    #[test]
    fn copy_rejects_absolute_src() {
        let result = Template::new().copy("/absolute/path", "/dest", CopyOpts::default());
        assert!(result.is_err(), "absolute src path must return Err");
    }

    #[test]
    fn copy_rejects_escaping_src() {
        let result = Template::new().copy("../escape", "/dest", CopyOpts::default());
        assert!(result.is_err(), "../escape must return Err");
    }

    #[test]
    fn copy_items_pushes_one_instruction_per_src() {
        let items = vec![crate::template::types::CopyItem {
            src: vec!["a.txt".to_string(), "b.txt".to_string()],
            dest: "/app/".to_string(),
            user: Some("u".to_string()),
            mode: Some(0o644),
            force_upload: None,
            resolve_symlinks: false,
        }];
        let t = Template::new().copy_items(items).expect("valid copy items");

        assert_eq!(t.instructions.len(), 2, "one instruction per src path");
        assert_eq!(t.instructions[0].args[0], "a.txt");
        assert_eq!(t.instructions[1].args[0], "b.txt");
        assert_eq!(t.instructions[0].args[3], "0644");
    }

    #[test]
    fn copy_items_rejects_absolute_src() {
        let items = vec![crate::template::types::CopyItem {
            src: vec!["/bad".to_string()],
            dest: "/app/".to_string(),
            ..Default::default()
        }];
        assert!(Template::new().copy_items(items).is_err());
    }

    #[test]
    fn remove_builds_rm() {
        let t = Template::new().remove(
            &["a", "b c"],
            RemoveOpts {
                recursive: true,
                force: true,
                user: Some("root".to_string()),
            },
        );

        assert_eq!(t.instructions.len(), 1);
        let instr = &t.instructions[0];
        assert_eq!(
            instr.instruction_type,
            crate::template::types::InstructionType::Run
        );
        // rm -r -f a 'b c'
        assert_eq!(instr.args[0], "rm -r -f a 'b c'");
        // user in args[1]
        assert_eq!(instr.args.get(1).map(String::as_str), Some("root"));
    }

    #[test]
    fn remove_no_flags_no_user() {
        let t = Template::new().remove(&["file.txt"], RemoveOpts::default());
        assert_eq!(t.instructions[0].args[0], "rm file.txt");
        assert_eq!(t.instructions[0].args.len(), 1);
    }

    #[test]
    fn rename_builds_mv() {
        // No force: mv <src> <dest>
        let t = Template::new().rename("old.txt", "new.txt", RenameOpts::default());
        assert_eq!(t.instructions[0].args[0], "mv old.txt new.txt");

        // With force: mv <src> <dest> -f  (JS SDK appends -f after operands)
        let t2 = Template::new().rename(
            "old.txt",
            "new.txt",
            RenameOpts {
                force: true,
                user: Some("admin".to_string()),
            },
        );
        assert_eq!(t2.instructions[0].args[0], "mv old.txt new.txt -f");
        assert_eq!(
            t2.instructions[0].args.get(1).map(String::as_str),
            Some("admin")
        );
    }

    #[test]
    fn rename_shell_quotes_paths_with_spaces() {
        let t = Template::new().rename("my file.txt", "my new.txt", RenameOpts::default());
        assert_eq!(t.instructions[0].args[0], "mv 'my file.txt' 'my new.txt'");
    }

    #[test]
    fn make_dir_builds_mkdir() {
        // With mode
        let t = Template::new().make_dir(
            &["/app/logs"],
            MakeDirOpts {
                mode: Some(0o755),
                user: None,
            },
        );
        assert_eq!(t.instructions[0].args[0], "mkdir -p -m 0755 /app/logs");

        // Without mode
        let t2 = Template::new().make_dir(&["/app/data", "/tmp/work"], MakeDirOpts::default());
        assert_eq!(t2.instructions[0].args[0], "mkdir -p /app/data /tmp/work");
    }

    #[test]
    fn make_dir_with_user() {
        let t = Template::new().make_dir(
            &["/home/user"],
            MakeDirOpts {
                user: Some("deployer".to_string()),
                mode: None,
            },
        );
        assert_eq!(
            t.instructions[0].args.get(1).map(String::as_str),
            Some("deployer")
        );
    }

    #[test]
    fn make_symlink_builds_ln() {
        // No force: ln -s <src> <dest>
        let t = Template::new().make_symlink("target", "link", MakeSymlinkOpts::default());
        assert_eq!(t.instructions[0].args[0], "ln -s target link");

        // With force: ln -s -f <src> <dest>
        let t2 = Template::new().make_symlink(
            "target",
            "link",
            MakeSymlinkOpts {
                force: true,
                user: Some("www".to_string()),
            },
        );
        assert_eq!(t2.instructions[0].args[0], "ln -s -f target link");
        assert_eq!(
            t2.instructions[0].args.get(1).map(String::as_str),
            Some("www")
        );
    }

    #[test]
    fn make_symlink_shell_quotes() {
        let t = Template::new().make_symlink(
            "/usr/local/bin/my app",
            "/usr/bin/my app",
            MakeSymlinkOpts::default(),
        );
        assert_eq!(
            t.instructions[0].args[0],
            "ln -s '/usr/local/bin/my app' '/usr/bin/my app'"
        );
    }

    #[test]
    fn push_run_inherits_template_force_flag() {
        let t = Template::new()
            .skip_cache()
            .remove(&["tmp"], RemoveOpts::default());
        // force is inherited from template.force = true
        assert!(t.instructions[0].force);
    }

    #[test]
    fn pad_octal_formats_correctly() {
        assert_eq!(pad_octal(0o755), "0755");
        assert_eq!(pad_octal(0o644), "0644");
        assert_eq!(pad_octal(0o000), "0000");
        assert_eq!(pad_octal(0o4755), "4755"); // setuid bit
    }

    // ── default cpu/mem defaults (Fix 2) ──────────────────────────────────────

    /// When no cpu or memory overrides are provided, the build request must
    /// include `cpuCount: 2` and `memoryMB: 1024` — matching the JS SDK
    /// default `cpuCount ?? 2, memoryMB ?? 1024`.
    #[tokio::test]
    async fn default_build_sends_cpu2_mem1024() {
        use wiremock::matchers::{body_partial_json, method, path};
        use wiremock::{Mock, MockServer, ResponseTemplate};

        let server = MockServer::start().await;

        Mock::given(method("POST"))
            .and(path("/v3/templates"))
            .and(body_partial_json(
                serde_json::json!({ "cpuCount": 2, "memoryMB": 1024 }),
            ))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "templateID": "tpl_def",
                "buildID": "bld_def",
                "aliases": [],
                "names": ["my-env"],
                "public": false,
                "tags": []
            })))
            .mount(&server)
            .await;

        Mock::given(method("POST"))
            .and(path("/v2/templates/tpl_def/builds/bld_def"))
            .respond_with(ResponseTemplate::new(204))
            .mount(&server)
            .await;

        let template = Template::new().from_image("node:20");
        let opts = BuildOptions {
            api_key: Some("e2b_0123456789abcdef".to_string()),
            api_url: Some(server.uri()),
            ..Default::default()
        };

        let info = template
            .build_in_background("my-env", opts)
            .await
            .expect("build_in_background with default cpu/mem should succeed");

        assert_eq!(info.template_id, "tpl_def");
    }

    // ── Task 3: run_cmd / run_cmds ────────────────────────────────────────────

    #[test]
    fn run_cmd_pushes_run_with_user() {
        let t = Template::new().run_cmd(
            "echo hello",
            RunCmdOpts {
                user: Some("root".to_string()),
            },
        );
        assert_eq!(t.instructions.len(), 1);
        let instr = &t.instructions[0];
        assert_eq!(instr.instruction_type, InstructionType::Run);
        // args[0] is the command, args[1] is the user
        assert_eq!(instr.args[0], "echo hello");
        assert_eq!(instr.args.get(1).map(String::as_str), Some("root"));
    }

    #[test]
    fn run_cmd_no_user_args_length_one() {
        let t = Template::new().run_cmd("ls -la", RunCmdOpts::default());
        assert_eq!(t.instructions[0].args.len(), 1);
        assert_eq!(t.instructions[0].args[0], "ls -la");
    }

    #[test]
    fn run_cmds_joins_with_and() {
        let t = Template::new().run_cmds(&["a", "b", "c"], RunCmdOpts::default());
        assert_eq!(t.instructions.len(), 1);
        assert_eq!(t.instructions[0].args[0], "a && b && c");
        assert_eq!(t.instructions[0].args.len(), 1); // no user
    }

    // ── Task 3: set_workdir / set_user ────────────────────────────────────────

    #[test]
    fn set_workdir_pushes_workdir_instruction() {
        let t = Template::new().set_workdir("/app");
        assert_eq!(t.instructions.len(), 1);
        let instr = &t.instructions[0];
        assert_eq!(instr.instruction_type, InstructionType::Workdir);
        assert_eq!(instr.args, vec!["/app"]);
    }

    #[test]
    fn set_user_pushes_user_instruction() {
        let t = Template::new().set_user("myuser");
        assert_eq!(t.instructions.len(), 1);
        let instr = &t.instructions[0];
        assert_eq!(instr.instruction_type, InstructionType::User);
        assert_eq!(instr.args, vec!["myuser"]);
    }

    // ── Task 3: set_envs ──────────────────────────────────────────────────────

    #[test]
    fn set_envs_flat_interleaved_args_sorted() {
        let mut envs = BTreeMap::new();
        envs.insert("Z".to_string(), "last".to_string());
        envs.insert("A".to_string(), "first".to_string());
        envs.insert("M".to_string(), "mid".to_string());

        let t = Template::new().set_envs(envs);
        assert_eq!(t.instructions.len(), 1);
        let instr = &t.instructions[0];
        assert_eq!(instr.instruction_type, InstructionType::Env);
        // BTreeMap order: A, M, Z
        assert_eq!(instr.args, vec!["A", "first", "M", "mid", "Z", "last"]);
    }

    #[test]
    fn set_envs_empty_map_no_instruction() {
        let t = Template::new().set_envs(BTreeMap::new());
        assert_eq!(
            t.instructions.len(),
            0,
            "empty envs must not push instruction"
        );
    }

    // ── Task 3: pip_install ───────────────────────────────────────────────────

    #[test]
    fn pip_install_default_global_no_packages_installs_dot() {
        // global=true (default), no packages → `pip install .` as root
        let t = Template::new().pip_install(&[], PipInstallOpts::default());
        let instr = &t.instructions[0];
        assert_eq!(instr.args[0], "pip install .");
        assert_eq!(instr.args.get(1).map(String::as_str), Some("root"));
    }

    #[test]
    fn pip_install_global_with_packages() {
        let t = Template::new().pip_install(
            &["requests", "flask"],
            PipInstallOpts::default(), // global=true
        );
        assert_eq!(t.instructions[0].args[0], "pip install requests flask");
        assert_eq!(
            t.instructions[0].args.get(1).map(String::as_str),
            Some("root")
        );
    }

    #[test]
    fn pip_install_user_flag_when_not_global() {
        let t = Template::new().pip_install(&["mypackage"], PipInstallOpts { global: false });
        assert_eq!(t.instructions[0].args[0], "pip install --user mypackage");
        // no user arg when global=false
        assert_eq!(t.instructions[0].args.len(), 1);
    }

    #[test]
    fn pip_install_user_flag_no_packages_installs_dot() {
        let t = Template::new().pip_install(&[], PipInstallOpts { global: false });
        assert_eq!(t.instructions[0].args[0], "pip install --user .");
        assert_eq!(t.instructions[0].args.len(), 1); // no user in args
    }

    // ── Task 3: npm_install ───────────────────────────────────────────────────

    #[test]
    fn npm_install_flags_global_and_dev() {
        let t = Template::new().npm_install(
            &["a", "b"],
            NpmInstallOpts {
                global: true,
                dev: true,
            },
        );
        assert_eq!(t.instructions[0].args[0], "npm install -g --save-dev a b");
        assert_eq!(
            t.instructions[0].args.get(1).map(String::as_str),
            Some("root")
        );
    }

    #[test]
    fn npm_install_no_flags_no_user() {
        let t = Template::new().npm_install(&["express"], NpmInstallOpts::default());
        assert_eq!(t.instructions[0].args[0], "npm install express");
        assert_eq!(t.instructions[0].args.len(), 1); // no user
    }

    // ── Task 3: bun_install ───────────────────────────────────────────────────

    #[test]
    fn bun_install_global_sets_root_user() {
        let t = Template::new().bun_install(
            &["a"],
            BunInstallOpts {
                global: true,
                dev: false,
            },
        );
        assert_eq!(t.instructions[0].args[0], "bun install -g a");
        assert_eq!(
            t.instructions[0].args.get(1).map(String::as_str),
            Some("root")
        );
    }

    #[test]
    fn bun_install_dev_flag() {
        let t = Template::new().bun_install(
            &["vitest"],
            BunInstallOpts {
                global: false,
                dev: true,
            },
        );
        assert_eq!(t.instructions[0].args[0], "bun install --dev vitest");
        assert_eq!(t.instructions[0].args.len(), 1); // no user when not global
    }

    // ── Task 3: apt_install ───────────────────────────────────────────────────

    #[test]
    fn apt_install_builds_command_with_all_flags() {
        let t = Template::new().apt_install(
            &["a", "b"],
            AptInstallOpts {
                no_install_recommends: true,
                fix_missing: true,
            },
        );
        assert_eq!(
            t.instructions[0].args[0],
            "apt-get update && DEBIAN_FRONTEND=noninteractive DEBCONF_NOWARNINGS=yes apt-get install -y --no-install-recommends --fix-missing a b"
        );
        assert_eq!(
            t.instructions[0].args.get(1).map(String::as_str),
            Some("root")
        );
    }

    #[test]
    fn apt_install_no_flags() {
        let t = Template::new().apt_install(&["curl"], AptInstallOpts::default());
        assert_eq!(
            t.instructions[0].args[0],
            "apt-get update && DEBIAN_FRONTEND=noninteractive DEBCONF_NOWARNINGS=yes apt-get install -y curl"
        );
        // always runs as root
        assert_eq!(
            t.instructions[0].args.get(1).map(String::as_str),
            Some("root")
        );
    }

    #[test]
    fn apt_install_only_no_install_recommends() {
        let t = Template::new().apt_install(
            &["git"],
            AptInstallOpts {
                no_install_recommends: true,
                fix_missing: false,
            },
        );
        assert_eq!(
            t.instructions[0].args[0],
            "apt-get update && DEBIAN_FRONTEND=noninteractive DEBCONF_NOWARNINGS=yes apt-get install -y --no-install-recommends git"
        );
    }

    // ── Task 3: git_clone ─────────────────────────────────────────────────────

    #[test]
    fn git_clone_full_with_all_options() {
        let t = Template::new().git_clone(
            "https://github.com/owner/repo.git",
            Some("/opt/repo"),
            GitCloneOpts {
                branch: Some("main".to_string()),
                depth: Some(1),
                user: Some("deployer".to_string()),
            },
        );
        assert_eq!(
            t.instructions[0].args[0],
            "git clone https://github.com/owner/repo.git --branch main --single-branch --depth 1 /opt/repo"
        );
        assert_eq!(
            t.instructions[0].args.get(1).map(String::as_str),
            Some("deployer")
        );
    }

    #[test]
    fn git_clone_minimal_no_options() {
        let t = Template::new().git_clone(
            "https://github.com/owner/repo.git",
            None,
            GitCloneOpts::default(),
        );
        assert_eq!(
            t.instructions[0].args[0],
            "git clone https://github.com/owner/repo.git"
        );
        assert_eq!(t.instructions[0].args.len(), 1); // no user
    }

    #[test]
    fn git_clone_url_with_spaces_gets_shell_quoted() {
        let t = Template::new().git_clone(
            "git@example.com:user/my repo.git",
            None,
            GitCloneOpts::default(),
        );
        // URL has a space → must be shell-quoted
        assert_eq!(
            t.instructions[0].args[0],
            "git clone 'git@example.com:user/my repo.git'"
        );
    }

    #[test]
    fn git_clone_branch_without_depth_or_path() {
        let t = Template::new().git_clone(
            "https://github.com/owner/repo.git",
            None,
            GitCloneOpts {
                branch: Some("feat/my-feature".to_string()),
                depth: None,
                user: None,
            },
        );
        // branch has `/` and `-`, which are safe shell chars → no quoting
        assert_eq!(
            t.instructions[0].args[0],
            "git clone https://github.com/owner/repo.git --branch feat/my-feature --single-branch"
        );
    }
}
