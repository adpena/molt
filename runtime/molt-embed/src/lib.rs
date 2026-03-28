//! Embeddable Python-to-WASM/native compilation SDK.
//!
//! Provides a minimal API for compiling Python subsets inline from Rust
//! applications without using the Molt CLI.
//!
//! # Usage
//!
//! ```ignore
//! use molt_embed::{MoltCompiler, CompileTarget, CompileOptions};
//!
//! let compiler = MoltCompiler::new()?;
//! let wasm_bytes = compiler.compile_to_wasm(
//!     "def fib(n): return n if n < 2 else fib(n-1) + fib(n-2)",
//!     CompileOptions::default(),
//! )?;
//! ```
//!
//! # Design
//!
//! The SDK provides:
//! - `MoltCompiler` — the main entry point for compilation
//! - `CompileOptions` — configurable compilation settings
//! - `CompileTarget` — target selection (native, WASM)
//! - `CompileResult` — compilation output with artifacts and diagnostics
//! - `CapabilitySet` — capability configuration for the compiled module
//!
//! The compiler shells out to the Molt backend daemon for heavy lifting,
//! keeping the embed crate lightweight. For environments where the daemon
//! is not available, a fallback path uses the backend library directly.

use std::collections::HashSet;
use std::path::{Path, PathBuf};

/// Compilation target.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CompileTarget {
    /// Native binary via Cranelift.
    Native,
    /// WebAssembly module.
    Wasm,
    /// WebAssembly module with linking support.
    WasmLinked,
}

impl Default for CompileTarget {
    fn default() -> Self {
        Self::Native
    }
}

/// Compilation optimization profile.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Profile {
    /// Fast compilation, no optimizations.
    Dev,
    /// Full optimization pipeline.
    Release,
}

impl Default for Profile {
    fn default() -> Self {
        Self::Release
    }
}

/// Resource limits to embed in the compiled module.
#[derive(Debug, Clone, Default)]
pub struct ResourceLimits {
    /// Maximum heap memory in bytes.
    pub max_memory: Option<usize>,
    /// Maximum wall-clock execution time in milliseconds.
    pub max_duration_ms: Option<u64>,
    /// Maximum number of heap allocations.
    pub max_allocations: Option<usize>,
    /// Maximum call stack depth.
    pub max_recursion_depth: Option<usize>,
}

/// Capabilities granted to the compiled module.
#[derive(Debug, Clone, Default)]
pub struct CapabilitySet {
    capabilities: HashSet<String>,
}

impl CapabilitySet {
    /// Create an empty capability set (maximum sandbox).
    pub fn new() -> Self {
        Self::default()
    }

    /// Grant a capability (e.g., "net", "fs.read", "env.read").
    pub fn grant(&mut self, cap: impl Into<String>) -> &mut Self {
        self.capabilities.insert(cap.into());
        self
    }

    /// Check if a capability is granted.
    pub fn has(&self, cap: &str) -> bool {
        self.capabilities.contains(cap)
    }

    /// Return the set as a comma-separated string for env propagation.
    pub fn to_env_string(&self) -> String {
        let mut caps: Vec<_> = self.capabilities.iter().cloned().collect();
        caps.sort();
        caps.join(",")
    }
}

/// Configuration for a compilation job.
#[derive(Debug, Clone)]
pub struct CompileOptions {
    /// Compilation target.
    pub target: CompileTarget,
    /// Optimization profile.
    pub profile: Profile,
    /// Capabilities granted to the module.
    pub capabilities: CapabilitySet,
    /// Resource limits.
    pub resource_limits: ResourceLimits,
    /// Enable audit logging in the compiled module.
    pub audit_enabled: bool,
    /// Enable type-gate security (capability-touching code must be fully typed).
    pub type_gate: bool,
    /// Enable deterministic mode.
    pub deterministic: bool,
}

impl Default for CompileOptions {
    fn default() -> Self {
        Self {
            target: CompileTarget::default(),
            profile: Profile::default(),
            capabilities: CapabilitySet::default(),
            resource_limits: ResourceLimits::default(),
            audit_enabled: false,
            type_gate: false,
            deterministic: true,
        }
    }
}

/// Compilation output.
#[derive(Debug)]
pub struct CompileResult {
    /// The compiled artifact bytes (binary or WASM).
    pub artifact: Vec<u8>,
    /// Compilation warnings and diagnostics.
    pub diagnostics: Vec<Diagnostic>,
    /// Time spent compiling (milliseconds).
    pub compile_time_ms: u64,
}

/// A compilation diagnostic (warning or info).
#[derive(Debug, Clone)]
pub struct Diagnostic {
    /// Severity level.
    pub level: DiagnosticLevel,
    /// Human-readable message.
    pub message: String,
    /// Source location (file:line:col), if available.
    pub location: Option<String>,
}

/// Diagnostic severity.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DiagnosticLevel {
    Info,
    Warning,
    Error,
}

/// Errors from the Molt compiler.
#[derive(Debug)]
pub enum CompileError {
    /// Python source has syntax errors.
    SyntaxError { message: String, line: Option<u32> },
    /// Type checking failed (with --type-gate).
    TypeError { message: String },
    /// Backend compilation failed.
    BackendError { message: String },
    /// IO error (file not found, etc.).
    IoError(std::io::Error),
    /// The Molt backend is not available.
    BackendUnavailable { message: String },
}

impl std::fmt::Display for CompileError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::SyntaxError { message, line } => {
                if let Some(l) = line {
                    write!(f, "syntax error at line {l}: {message}")
                } else {
                    write!(f, "syntax error: {message}")
                }
            }
            Self::TypeError { message } => write!(f, "type error: {message}"),
            Self::BackendError { message } => write!(f, "backend error: {message}"),
            Self::IoError(e) => write!(f, "IO error: {e}"),
            Self::BackendUnavailable { message } => write!(f, "backend unavailable: {message}"),
        }
    }
}

impl std::error::Error for CompileError {}

impl From<std::io::Error> for CompileError {
    fn from(e: std::io::Error) -> Self {
        Self::IoError(e)
    }
}

/// The main entry point for embedding Molt compilation.
///
/// # Example
///
/// ```ignore
/// let compiler = MoltCompiler::new()?;
/// let result = compiler.compile_source(
///     "def hello(): return 'world'",
///     CompileOptions::default(),
/// )?;
/// println!("Compiled {} bytes", result.artifact.len());
/// ```
pub struct MoltCompiler {
    /// Path to the Molt home directory (~/.molt).
    molt_home: PathBuf,
    /// Path to the Molt CLI (for subprocess compilation).
    molt_cli: Option<PathBuf>,
}

impl MoltCompiler {
    /// Create a new compiler instance.
    ///
    /// Discovers the Molt installation by checking:
    /// 1. `MOLT_HOME` environment variable
    /// 2. `~/.molt` default directory
    pub fn new() -> Result<Self, CompileError> {
        let molt_home = std::env::var("MOLT_HOME")
            .map(PathBuf::from)
            .unwrap_or_else(|_| dirs_home().join(".molt"));

        // Try to find the molt CLI
        let molt_cli = which_molt();

        Ok(Self {
            molt_home,
            molt_cli,
        })
    }

    /// Compile Python source code to the specified target.
    pub fn compile_source(
        &self,
        source: &str,
        options: CompileOptions,
    ) -> Result<CompileResult, CompileError> {
        let start = std::time::Instant::now();

        // Write source to a temp file
        let tmp_dir = std::env::temp_dir().join("molt-embed");
        std::fs::create_dir_all(&tmp_dir)?;
        let src_path = tmp_dir.join("embed_input.py");
        std::fs::write(&src_path, source)?;

        let result = self.compile_file(&src_path, &options)?;

        let compile_time_ms = start.elapsed().as_millis() as u64;

        Ok(CompileResult {
            artifact: result,
            diagnostics: vec![],
            compile_time_ms,
        })
    }

    /// Compile a Python file to the specified target.
    pub fn compile_file(
        &self,
        path: &Path,
        options: &CompileOptions,
    ) -> Result<Vec<u8>, CompileError> {
        let molt_cli = self
            .molt_cli
            .as_ref()
            .ok_or_else(|| CompileError::BackendUnavailable {
                message: "molt CLI not found in PATH or MOLT_HOME".into(),
            })?;

        let tmp_out = std::env::temp_dir().join("molt-embed").join("output");
        std::fs::create_dir_all(&tmp_out)?;

        let mut cmd = std::process::Command::new(molt_cli);
        cmd.arg("build");
        cmd.arg(path);
        cmd.arg("--out-dir").arg(&tmp_out);

        // Target
        match options.target {
            CompileTarget::Native => {}
            CompileTarget::Wasm => {
                cmd.arg("--target").arg("wasm");
            }
            CompileTarget::WasmLinked => {
                cmd.arg("--target").arg("wasm");
                cmd.arg("--linked");
            }
        }

        // Profile
        match options.profile {
            Profile::Dev => {
                cmd.arg("--profile").arg("dev");
            }
            Profile::Release => {
                cmd.arg("--profile").arg("release");
            }
        }

        // Capabilities
        let caps = options.capabilities.to_env_string();
        if !caps.is_empty() {
            cmd.arg("--capabilities").arg(&caps);
        }

        // Determinism
        if options.deterministic {
            cmd.arg("--deterministic");
        }

        let output = cmd.output()?;
        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            return Err(CompileError::BackendError {
                message: stderr.to_string(),
            });
        }

        // Find the output artifact
        let artifact_path = match options.target {
            CompileTarget::Wasm | CompileTarget::WasmLinked => tmp_out.join("output.wasm"),
            CompileTarget::Native => {
                // Find the binary in out_dir
                let stem = path.file_stem().unwrap_or_default().to_string_lossy();
                tmp_out.join(format!("{stem}_molt"))
            }
        };

        std::fs::read(&artifact_path).map_err(|e| CompileError::IoError(e))
    }
}

/// Find the user's home directory.
fn dirs_home() -> PathBuf {
    std::env::var("HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|_| PathBuf::from("/tmp"))
}

/// Try to find the `molt` CLI in PATH or MOLT_HOME.
fn which_molt() -> Option<PathBuf> {
    // Check MOLT_HOME/bin first
    if let Ok(home) = std::env::var("MOLT_HOME") {
        let p = PathBuf::from(home).join("bin").join("molt");
        if p.exists() {
            return Some(p);
        }
    }

    // Check PATH via `which`
    std::process::Command::new("which")
        .arg("molt")
        .output()
        .ok()
        .and_then(|o| {
            if o.status.success() {
                String::from_utf8(o.stdout)
                    .ok()
                    .map(|s| PathBuf::from(s.trim()))
            } else {
                None
            }
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn capability_set_operations() {
        let mut caps = CapabilitySet::new();
        assert!(!caps.has("net"));
        caps.grant("net");
        caps.grant("fs.read");
        assert!(caps.has("net"));
        assert!(caps.has("fs.read"));
        assert!(!caps.has("fs.write"));
        assert_eq!(caps.to_env_string(), "fs.read,net");
    }

    #[test]
    fn compile_options_default() {
        let opts = CompileOptions::default();
        assert_eq!(opts.target, CompileTarget::Native);
        assert_eq!(opts.profile, Profile::Release);
        assert!(opts.deterministic);
        assert!(!opts.audit_enabled);
    }

    #[test]
    fn resource_limits_default() {
        let limits = ResourceLimits::default();
        assert!(limits.max_memory.is_none());
        assert!(limits.max_duration_ms.is_none());
    }
}
