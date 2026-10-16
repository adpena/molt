//! One environment catalog for frontend identities, daemon request custody,
//! native worker inputs and diagnostic cache bypass. No cache-local knob list.
//!
//! Compiler crates read backend settings only through [`var`], [`var_os`] and
//! [`is_set`], which keep `std::env`'s contracts, keyed by the closed
//! [`BackendEnv`] type. A one-shot backend takes them from its process
//! environment. The daemon serves many builds at once, so it never writes a
//! request's settings into the shared process environment: each request
//! installs its own [`BackendEnvironment`] on the compiling thread ([`enter`]),
//! and work handed to other threads carries it along ([`carry`]). sccache sends
//! each compile's environment with its request the same way, and rustc's
//! thread pool carries a job's context into its workers
//! (`docs/design/backend_compile_daemon.md`).

use std::cell::RefCell;
use std::collections::BTreeMap;
use std::ffi::OsString;
use std::io;
use std::process::Command;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

include!(concat!(env!("OUT_DIR"), "/backend_environment_keys.rs"));

impl BackendEnvGroup {
    /// Whether a daemon request carries this group. Build identity names the
    /// compiler binary, which the daemon's config digest fixes for its whole
    /// life, so it stays in the process environment.
    pub const fn request_scoped(self) -> bool {
        match self {
            BackendEnvGroup::BuildIdentity => false,
            BackendEnvGroup::Common
            | BackendEnvGroup::Native
            | BackendEnvGroup::RuntimeIdentity
            | BackendEnvGroup::Wasm
            | BackendEnvGroup::Diagnostic
            | BackendEnvGroup::Observation
            | BackendEnvGroup::Resource
            | BackendEnvGroup::Transport => true,
        }
    }

    /// Whether the group selects native code generation, so a serialized
    /// native worker job binds it.
    const fn native_codegen_identity(self) -> bool {
        match self {
            BackendEnvGroup::Common
            | BackendEnvGroup::Native
            | BackendEnvGroup::RuntimeIdentity
            | BackendEnvGroup::BuildIdentity => true,
            BackendEnvGroup::Wasm
            | BackendEnvGroup::Diagnostic
            | BackendEnvGroup::Observation
            | BackendEnvGroup::Resource
            | BackendEnvGroup::Transport => false,
        }
    }
}

/// The variables a daemon request carries.
pub fn daemon_request_env_keys() -> impl Iterator<Item = BackendEnv> {
    BackendEnv::ALL
        .iter()
        .copied()
        .filter(|key| key.group().request_scoped())
}

/// One request's backend settings. Absent and empty values stay distinct:
/// several pass controls test presence, not truthiness.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct BackendEnvironment {
    values: BTreeMap<BackendEnv, Box<str>>,
}

impl BackendEnvironment {
    /// Admits a daemon request's `env` object against the closed key set.
    pub fn from_request(
        object: &serde_json::Map<String, serde_json::Value>,
    ) -> Result<Self, String> {
        let mut values = BTreeMap::new();
        for (name, value) in object {
            let key = BackendEnv::from_name(name)
                .ok_or_else(|| format!("unknown backend environment variable {name}"))?;
            if !key.group().request_scoped() {
                return Err(format!(
                    "{name} is fixed for the daemon's lifetime and cannot vary per request"
                ));
            }
            let value = value
                .as_str()
                .ok_or_else(|| format!("backend environment variable {name} must be a string"))?;
            values.insert(key, value.into());
        }
        Ok(Self { values })
    }

    /// A snapshot with exactly these settings.
    #[cfg(test)]
    pub fn from_values<'a>(values: impl IntoIterator<Item = (BackendEnv, &'a str)>) -> Self {
        Self {
            values: values
                .into_iter()
                .map(|(key, value)| (key, value.into()))
                .collect(),
        }
    }

    pub fn get(&self, key: BackendEnv) -> Option<&str> {
        self.values.get(&key).map(|value| &**value)
    }
}

thread_local! {
    static SCOPE: RefCell<Option<Arc<BackendEnvironment>>> = const { RefCell::new(None) };
}

static REQUEST_SCOPED_PROCESS: AtomicBool = AtomicBool::new(false);

/// Makes this process a request server: request-scoped settings come only
/// from an entered request scope, and a read outside one panics, because it
/// would otherwise see another build's settings or none. The process
/// environment loses its request-scoped variables, so a direct read cannot
/// see stale ones and a spawned child inherits none.
///
/// # Safety
///
/// No other thread may read or write the process environment while this
/// runs; call it at startup, before the server spawns threads.
pub unsafe fn serve_requests() {
    for key in daemon_request_env_keys() {
        // SAFETY: the caller guarantees this process is still single-threaded.
        unsafe { std::env::remove_var(key.name()) };
    }
    REQUEST_SCOPED_PROCESS.store(true, Ordering::Release);
}

/// Restores the previous request scope when dropped.
#[must_use = "the request scope ends when the guard drops"]
pub struct ScopeGuard {
    previous: Option<Arc<BackendEnvironment>>,
}

impl Drop for ScopeGuard {
    fn drop(&mut self) {
        let previous = self.previous.take();
        SCOPE.with(|scope| *scope.borrow_mut() = previous);
    }
}

/// Makes `environment` this thread's request scope until the guard drops.
pub fn enter(environment: Arc<BackendEnvironment>) -> ScopeGuard {
    let previous = SCOPE.with(|scope| scope.borrow_mut().replace(environment));
    ScopeGuard { previous }
}

/// The current thread's request scope, to hand to worker threads.
#[derive(Clone, Default)]
pub struct Carried(Option<Arc<BackendEnvironment>>);

/// Captures this thread's request scope for work that runs on other threads.
pub fn carry() -> Carried {
    Carried(SCOPE.with(|scope| scope.borrow().clone()))
}

impl Carried {
    /// Runs `work` under the captured request scope.
    pub fn enter<R>(&self, work: impl FnOnce() -> R) -> R {
        let _guard = self.0.clone().map(enter);
        work()
    }
}

/// Looks `key` up in the request scope; `None` means there is no scope and
/// the process environment is the source.
fn scoped<R>(key: BackendEnv, read: impl FnOnce(Option<&str>) -> R) -> Option<R> {
    if !key.group().request_scoped() {
        return None;
    }
    let found = SCOPE.with(|scope| scope.borrow().as_ref().map(|env| read(env.get(key))));
    if found.is_none() && REQUEST_SCOPED_PROCESS.load(Ordering::Acquire) {
        panic!(
            "{} read outside a backend request scope; carry the scope into this thread",
            key.name()
        );
    }
    found
}

/// The process environment's value: the one-shot backend's settings.
#[expect(
    clippy::disallowed_methods,
    reason = "the accessor itself reads the process environment outside a request"
)]
fn process_var_os(key: BackendEnv) -> Option<OsString> {
    std::env::var_os(key.name())
}

/// The setting's value, with `std::env::var`'s contract.
pub fn var(key: BackendEnv) -> Result<String, std::env::VarError> {
    match scoped(key, |value| value.map(str::to_owned)) {
        Some(Some(value)) => Ok(value),
        Some(None) => Err(std::env::VarError::NotPresent),
        None => process_var_os(key)
            .ok_or(std::env::VarError::NotPresent)?
            .into_string()
            .map_err(std::env::VarError::NotUnicode),
    }
}

/// The setting's raw value, for paths.
pub fn var_os(key: BackendEnv) -> Option<OsString> {
    scoped(key, |value| value.map(OsString::from)).unwrap_or_else(|| process_var_os(key))
}

/// Whether the setting is present, empty or not.
pub fn is_set(key: BackendEnv) -> bool {
    scoped(key, |value| value.is_some()).unwrap_or_else(|| process_var_os(key).is_some())
}

/// The setting's value, failing on a non-Unicode process value.
fn try_var(key: BackendEnv) -> io::Result<Option<String>> {
    match var(key) {
        Ok(value) => Ok(Some(value)),
        Err(std::env::VarError::NotPresent) => Ok(None),
        Err(std::env::VarError::NotUnicode(_)) => Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            format!("non-Unicode backend setting {}", key.name()),
        )),
    }
}

/// Gives a child backend process exactly the current request's settings. In
/// a one-shot process the child already inherits them.
pub fn configure_child(command: &mut Command) {
    let Carried(Some(environment)) = carry() else {
        return;
    };
    for key in daemon_request_env_keys() {
        match environment.get(key) {
            Some(value) => command.env(key.name(), value),
            None => command.env_remove(key.name()),
        };
    }
}

/// Dumps, validators and pass instruments require actual compilation. Timing
/// and artifact-directory settings alone remain compatible with measured hits.
pub fn compilation_diagnostics_requested() -> bool {
    BackendEnv::ALL
        .iter()
        .filter(|key| key.group() == BackendEnvGroup::Diagnostic)
        .any(|key| is_set(*key))
}

#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct NativeCodegenEnvironment {
    schema: u32,
    pub values: BTreeMap<String, Option<String>>,
}

const NATIVE_CODEGEN_ENVIRONMENT_SCHEMA: u32 = 1;

impl NativeCodegenEnvironment {
    pub fn capture() -> io::Result<Self> {
        Self::from_lookup(try_var)
    }

    pub fn from_lookup(
        mut lookup: impl FnMut(BackendEnv) -> io::Result<Option<String>>,
    ) -> io::Result<Self> {
        let values = BackendEnv::ALL
            .iter()
            .filter(|key| key.group().native_codegen_identity())
            .map(|key| lookup(*key).map(|value| (key.name().to_string(), value)))
            .collect::<io::Result<_>>()?;
        Ok(Self {
            schema: NATIVE_CODEGEN_ENVIRONMENT_SCHEMA,
            values,
        })
    }

    /// The serialized job binds exactly the environment inherited by its worker.
    /// Replay rejects drift instead of silently compiling a different contract.
    pub fn validate_current(&self) -> io::Result<()> {
        let current = Self::capture()?;
        if self == &current {
            return Ok(());
        }
        let changed: Vec<_> = current
            .values
            .keys()
            .filter(|name| self.values.get(*name) != current.values.get(*name))
            .collect();
        Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            format!(
                "native worker codegen environment differs from serialized job (schema {} vs {}): {changed:?}",
                self.schema, current.schema,
            ),
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn presence_and_emitter_controls_are_distinct_serialized_inputs() {
        let absent = NativeCodegenEnvironment::from_lookup(|_| Ok(None)).unwrap();
        for (key, value) in [
            (BackendEnv::MOLT_DISABLE_RC_COALESCE, ""),
            (BackendEnv::MOLT_BACKEND_INLINE_EXC_DISABLED, "1"),
        ] {
            let changed = NativeCodegenEnvironment::from_lookup(|probe| {
                Ok((probe == key).then(|| value.to_string()))
            })
            .unwrap();
            assert_ne!(absent, changed);
            assert!(daemon_request_env_keys().any(|request_key| request_key == key));
        }
        assert_eq!(BackendEnv::from_name("MOLT_DISABLE_RC_COALESCING"), None);
    }

    #[test]
    fn names_round_trip_through_the_closed_key_set() {
        for key in BackendEnv::ALL {
            assert_eq!(BackendEnv::from_name(key.name()), Some(*key));
        }
    }

    #[test]
    fn requests_admit_only_request_scoped_catalog_strings() {
        let parse = |value: serde_json::Value| {
            BackendEnvironment::from_request(value.as_object().expect("object"))
        };
        let env =
            parse(serde_json::json!({"MOLT_ENTRY_MODULE": "app", "MOLT_DISABLE_RC_COALESCE": ""}))
                .expect("catalog strings");
        assert_eq!(env.get(BackendEnv::MOLT_ENTRY_MODULE), Some("app"));
        assert_eq!(env.get(BackendEnv::MOLT_DISABLE_RC_COALESCE), Some(""));
        assert_eq!(env.get(BackendEnv::MOLT_WASM_PROFILE), None);
        assert!(parse(serde_json::json!({"MOLT_NOT_A_SETTING": "1"})).is_err());
        assert!(parse(serde_json::json!({"MOLT_ENTRY_MODULE": 1})).is_err());
        assert!(parse(serde_json::json!({"MOLT_BACKEND_COMPILER_FINGERPRINT": "x"})).is_err());
    }

    #[test]
    fn scopes_isolate_threads_and_carry_into_workers() {
        let a = Arc::new(BackendEnvironment::from_values([(
            BackendEnv::MOLT_ENTRY_MODULE,
            "a",
        )]));
        let b = Arc::new(BackendEnvironment::from_values([(
            BackendEnv::MOLT_ENTRY_MODULE,
            "b",
        )]));
        let barrier = Arc::new(std::sync::Barrier::new(2));
        let run = |env: Arc<BackendEnvironment>, expected: &'static str| {
            let barrier = Arc::clone(&barrier);
            std::thread::spawn(move || {
                let _scope = enter(env);
                // Both threads hold their scopes at once.
                barrier.wait();
                assert_eq!(var(BackendEnv::MOLT_ENTRY_MODULE).as_deref(), Ok(expected));
                let carried = carry();
                std::thread::spawn(move || {
                    carried.enter(|| var(BackendEnv::MOLT_ENTRY_MODULE).ok())
                })
                .join()
                .expect("worker")
            })
        };
        let (left, right) = (run(a, "a"), run(b, "b"));
        assert_eq!(left.join().expect("a").as_deref(), Some("a"));
        assert_eq!(right.join().expect("b").as_deref(), Some("b"));
    }

    #[test]
    fn a_child_process_gets_exactly_its_request_settings() {
        let envs = |command: &Command| -> BTreeMap<String, Option<String>> {
            command
                .get_envs()
                .map(|(name, value)| {
                    (
                        name.to_string_lossy().into_owned(),
                        value.map(|value| value.to_string_lossy().into_owned()),
                    )
                })
                .collect()
        };
        // Outside a request the child inherits this process's settings.
        let mut command = Command::new("molt-backend");
        configure_child(&mut command);
        assert!(envs(&command).is_empty());

        let _scope = enter(Arc::new(BackendEnvironment::from_values([(
            BackendEnv::MOLT_ENTRY_MODULE,
            "app",
        )])));
        let mut command = Command::new("molt-backend");
        configure_child(&mut command);
        let envs = envs(&command);
        assert_eq!(
            envs.get("MOLT_ENTRY_MODULE"),
            Some(&Some("app".to_string()))
        );
        assert_eq!(
            envs.get("MOLT_STDLIB_OBJ"),
            Some(&None),
            "absent settings are removed"
        );
        assert!(
            !envs.contains_key("MOLT_BACKEND_COMPILER_FINGERPRINT"),
            "build identity stays with the process"
        );
        assert_eq!(envs.len(), daemon_request_env_keys().count());
    }

    #[test]
    fn a_scope_hides_absent_settings_and_restores_on_exit() {
        let env = Arc::new(BackendEnvironment::default());
        {
            let _scope = enter(env);
            assert!(!is_set(BackendEnv::MOLT_ENTRY_MODULE));
            assert!(var(BackendEnv::MOLT_ENTRY_MODULE).is_err());
        }
        assert!(carry().0.is_none());
    }
}
