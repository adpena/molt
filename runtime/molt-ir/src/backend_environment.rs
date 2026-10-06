//! One environment catalog for frontend identities, daemon request custody,
//! native worker inputs and diagnostic cache bypass. No cache-local knob list.

use std::collections::BTreeMap;
use std::io;
use std::sync::LazyLock;

#[derive(serde::Deserialize)]
struct Catalog {
    schema: u32,
    #[serde(flatten)]
    groups: BTreeMap<String, Vec<String>>,
}

static CATALOG: LazyLock<Catalog> = LazyLock::new(|| {
    let catalog: Catalog =
        serde_json::from_str(include_str!("../../../src/molt/backend_environment.json"))
            .expect("valid shared backend environment catalog");
    assert_eq!(catalog.schema, 1, "unsupported backend environment catalog");
    let expected: std::collections::BTreeSet<_> = [
        "common",
        "native",
        "wasm",
        "diagnostic",
        "observation",
        "resource",
        "transport",
        "runtime_identity",
        "build_identity",
    ]
    .into_iter()
    .collect();
    assert_eq!(
        catalog
            .groups
            .keys()
            .map(String::as_str)
            .collect::<std::collections::BTreeSet<_>>(),
        expected,
        "malformed backend environment catalog groups"
    );
    let mut names = std::collections::BTreeSet::new();
    for name in catalog.groups.values().flatten() {
        assert!(
            !name.is_empty()
                && name
                    .bytes()
                    .all(|byte| byte.is_ascii_uppercase() || byte.is_ascii_digit() || byte == b'_'),
            "malformed backend environment name {name}"
        );
        assert!(
            names.insert(name),
            "duplicate backend environment catalog entry {name}"
        );
    }
    catalog
});

fn group(name: &str) -> &'static [String] {
    CATALOG
        .groups
        .get(name)
        .expect("known backend environment group")
}

pub fn daemon_request_env_keys() -> &'static [&'static str] {
    static KEYS: LazyLock<Vec<&'static str>> = LazyLock::new(|| {
        CATALOG
            .groups
            .iter()
            .filter(|(name, _)| name.as_str() != "build_identity")
            .flat_map(|(_, names)| names)
            .map(String::as_str)
            .collect()
    });
    &KEYS
}

/// Dumps, validators and pass instruments require actual compilation. Timing
/// and artifact-directory settings alone remain compatible with measured hits.
pub fn compilation_diagnostics_requested() -> bool {
    group("diagnostic")
        .iter()
        .any(|name| std::env::var_os(name).is_some())
}

#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct NativeCodegenEnvironment {
    schema: u32,
    pub values: BTreeMap<String, Option<String>>,
}

impl NativeCodegenEnvironment {
    pub fn capture() -> io::Result<Self> {
        Self::from_lookup(|name| match std::env::var(name) {
            Ok(value) => Ok(Some(value)),
            Err(std::env::VarError::NotPresent) => Ok(None),
            Err(std::env::VarError::NotUnicode(_)) => Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                format!("non-Unicode codegen setting {name}"),
            )),
        })
    }

    pub fn from_lookup(
        mut lookup: impl FnMut(&str) -> io::Result<Option<String>>,
    ) -> io::Result<Self> {
        let values = group("common")
            .iter()
            .chain(group("native"))
            .chain(group("runtime_identity"))
            .chain(group("build_identity"))
            .map(|name| lookup(name).map(|value| (name.clone(), value)))
            .collect::<io::Result<_>>()?;
        Ok(Self {
            schema: CATALOG.schema,
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
        for (name, value) in [
            ("MOLT_DISABLE_RC_COALESCE", ""),
            ("MOLT_BACKEND_INLINE_EXC_DISABLED", "1"),
        ] {
            let changed = NativeCodegenEnvironment::from_lookup(|key| {
                Ok((key == name).then(|| value.to_string()))
            })
            .unwrap();
            assert_ne!(absent, changed);
            assert!(daemon_request_env_keys().contains(&name));
        }
        assert!(!daemon_request_env_keys().contains(&"MOLT_DISABLE_RC_COALESCING"));
    }
}
