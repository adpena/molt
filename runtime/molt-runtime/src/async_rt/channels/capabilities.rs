use std::collections::HashSet;

use crate::{PyToken, runtime_state};

// Three tiers replace the 80+ individual capability names for DX:
//
//   safe      Read-only. No network, no writes, no exec. Default for
//             `molt deploy --cloudflare` and sandboxed environments.
//
//   standard  Adds filesystem writes, env writes, temp files, process
//             signals, time access. Default for `molt run` in dev.
//
//   full      Everything except exec/eval/monkeypatching (which Molt
//             never supports). Equivalent to MOLT_TRUSTED=1.
//
// Set via: MOLT_CAPABILITY_TIER=safe|standard|full
// Override individual caps: MOLT_CAPABILITIES=cap1,cap2,...  (additive)
// Legacy: MOLT_TRUSTED=1 is equivalent to tier=full.

/// Capabilities granted at the `safe` tier (read-only operations).
const TIER_SAFE: &[&str] = &[
    "env.read",
    "env.len",
    "env.snapshot",
    "fs.read",
    "fs.stat",
    "fs.readdir",
    "glob.glob",
    "os.access",
    "os.getcwd",
    "os.getpid",
    "os.getppid",
    "os.getuid",
    "os.geteuid",
    "os.getgid",
    "os.getegid",
    "os.getlogin",
    "os.getpgrp",
    "os.getloadavg",
    "os.uname",
    "os.readlink",
    "os.listdir",
    "os.scandir",
    "os.walk",
    "shutil.which",
    "time.wall",
];

/// Additional capabilities granted at the `standard` tier.
const TIER_STANDARD_EXTRA: &[&str] = &[
    "env.write",
    "env.clear",
    "env.popitem",
    "env.expanduser",
    "env.expandvars",
    "fs.write",
    "os.mkdir",
    "os.makedirs",
    "os.rmdir",
    "os.removedirs",
    "os.link",
    "os.symlink",
    "os.chmod",
    "os.utime",
    "os.chdir",
    "os.umask",
    "os.truncate",
    "os.ftruncate",
    "os.lseek",
    "shutil.copy",
    "shutil.copyfile",
    "shutil.copytree",
    "shutil.move",
    "tempfile.mkdtemp",
    "tempfile.mkstemp",
    "tempfile.named",
    "tempfile.tempdir",
    "tempfile.cleanup",
    "signal.signal",
    "signal.raise",
    "signal.alarm",
    "signal.pause",
    "thread.spawn",
    "thread.shared",
];

/// Additional capabilities granted at the `full` tier.
const TIER_FULL_EXTRA: &[&str] = &[
    "net.bind",
    "net.listen",
    "net.connect",
    "net.poll",
    "net.asyncio",
    "ssl.read",
    "ssl.write",
    "websocket.connect",
    "process.exec",
    "process.asyncio",
    "os.kill",
    "os.waitpid",
    "os.sendfile",
    "os.setpgrp",
    "os.setsid",
    "select.select",
    "select.poll",
    "select.epoll",
    "select.devpoll",
    "db.read",
    "db.write",
    "db.query",
    "db.exec",
    "ffi.unsafe",
    "ffi.require",
    "ffi.sizeof",
    "fcntl.fcntl",
    "python.bridge",
];

fn load_capabilities() -> HashSet<String> {
    let tier = std::env::var("MOLT_CAPABILITY_TIER")
        .ok()
        .map(|t| t.trim().to_ascii_lowercase())
        .unwrap_or_else(|| "standard".to_string());

    let mut set: HashSet<String> = HashSet::new();
    for &cap in TIER_SAFE {
        set.insert(cap.to_string());
    }
    if tier == "standard" || tier == "full" {
        for &cap in TIER_STANDARD_EXTRA {
            set.insert(cap.to_string());
        }
    }
    if tier == "full" {
        for &cap in TIER_FULL_EXTRA {
            set.insert(cap.to_string());
        }
    }

    let caps = std::env::var("MOLT_CAPABILITIES").unwrap_or_default();
    for cap in caps.split(',') {
        let cap = cap.trim();
        if !cap.is_empty() {
            set.insert(cap.to_string());
        }
    }
    set
}

fn load_trusted() -> bool {
    match std::env::var("MOLT_TRUSTED") {
        Ok(val) => matches!(
            val.trim().to_ascii_lowercase().as_str(),
            "1" | "true" | "yes" | "on"
        ),
        Err(_) => false,
    }
}

pub(crate) fn is_trusted(_py: &PyToken<'_>) -> bool {
    *runtime_state(_py).trusted.get_or_init(load_trusted)
}

pub(crate) fn has_capability(_py: &PyToken<'_>, name: &str) -> bool {
    if is_trusted(_py) {
        return true;
    }
    let caps = runtime_state(_py)
        .capabilities
        .get_or_init(load_capabilities);
    caps.contains(name)
}

/// Suggest the minimum tier or env var needed to grant a missing capability.
/// Raise a PermissionError with an actionable message including the
/// capability name, tier suggestion, and `--trusted` escape hatch.
pub(crate) fn raise_capability_denied<T: crate::builtins::exceptions::ExceptionSentinel>(
    _py: &crate::concurrency::PyToken<'_>,
    cap: &str,
) -> T {
    let hint = capability_fix_hint(cap);
    crate::raise_exception::<T>(
        _py,
        "PermissionError",
        &format!("missing '{cap}' capability. {hint}"),
    )
}

/// Suggest the minimum tier or env var needed to grant a missing capability.
pub(crate) fn capability_fix_hint(name: &str) -> String {
    if TIER_SAFE.contains(&name) {
        return format!(
            "'{name}' should be in the default tier. Set MOLT_CAPABILITIES={name} or MOLT_TRUSTED=1"
        );
    }
    if TIER_STANDARD_EXTRA.contains(&name) {
        return "Use --trusted, MOLT_TRUSTED=1, or MOLT_CAPABILITY_TIER=standard (default for molt run)".to_string();
    }
    if TIER_FULL_EXTRA.contains(&name) {
        return "Use --trusted, MOLT_TRUSTED=1, or MOLT_CAPABILITY_TIER=full".to_string();
    }
    format!("Use --trusted, MOLT_TRUSTED=1, or MOLT_CAPABILITIES={name}")
}
