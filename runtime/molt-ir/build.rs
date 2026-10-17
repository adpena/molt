//! Generate the closed environment keys and roles from the one Python/Rust catalog.
#![expect(
    clippy::disallowed_methods,
    reason = "Cargo supplies build-script directories"
)]

use std::collections::{BTreeMap, BTreeSet};
use std::fmt::Write as _;
use std::path::PathBuf;

fn variant(name: &str) -> String {
    name.split('_')
        .map(|part| {
            let mut chars = part.chars();
            chars.next().unwrap().to_ascii_uppercase().to_string() + chars.as_str()
        })
        .collect()
}

fn main() {
    let manifest = PathBuf::from(std::env::var_os("CARGO_MANIFEST_DIR").expect("Cargo manifest"));
    let path = manifest.join("../../src/molt/backend_environment.json");
    println!("cargo:rerun-if-changed={}", path.display());
    let text = std::fs::read_to_string(&path).expect("read backend environment catalog");
    let catalog: serde_json::Value =
        serde_json::from_str(&text).expect("parse environment catalog");
    let catalog = catalog.as_object().expect("environment catalog object");
    assert_eq!(catalog.len(), 2, "unknown environment catalog fields");
    assert_eq!(
        catalog.get("schema").and_then(|v| v.as_u64()),
        Some(2),
        "unsupported environment catalog schema"
    );
    let groups = catalog
        .get("groups")
        .and_then(|v| v.as_object())
        .expect("catalog groups");
    assert!(!groups.is_empty(), "empty environment catalog");
    let fields = [
        "keys",
        "request_scoped",
        "semantic_target",
        "native_worker",
        "projection",
        "diagnostic_bypass",
        "request_reuse_only",
    ];
    let mut seen = BTreeSet::new();
    let mut members = BTreeMap::new();
    for (group, role) in groups {
        assert!(
            group
                .split('_')
                .all(|part| !part.is_empty() && part.bytes().all(|b| b.is_ascii_lowercase())),
            "malformed environment group"
        );
        let role = role.as_object().expect("environment role object");
        assert!(
            role.len() == fields.len() && fields.iter().all(|field| role.contains_key(*field)),
            "unknown or missing environment role fields"
        );
        for field in [
            "request_scoped",
            "native_worker",
            "diagnostic_bypass",
            "request_reuse_only",
        ] {
            assert!(role[field].is_boolean(), "{field} must be boolean");
        }
        assert!(
            matches!(
                role["semantic_target"].as_str(),
                Some("native" | "wasm" | "both" | "none")
            ),
            "unknown semantic target"
        );
        assert!(
            matches!(role["projection"].as_str(), Some("raw" | "presence")),
            "unknown environment projection"
        );
        assert!(
            !role["request_reuse_only"].as_bool().unwrap()
                || (role["request_scoped"].as_bool().unwrap()
                    && !role["native_worker"].as_bool().unwrap()
                    && role["semantic_target"].as_str() != Some("none")),
            "contradictory reuse-only environment role"
        );
        assert!(
            !role["native_worker"].as_bool().unwrap()
                || matches!(role["semantic_target"].as_str(), Some("native" | "both")),
            "native worker role requires native semantics"
        );
        let keys = role["keys"].as_array().expect("environment keys array");
        assert!(!keys.is_empty(), "empty environment group");
        let mut names = Vec::new();
        for key in keys {
            let key = key.as_str().expect("environment key string");
            assert!(
                !key.is_empty()
                    && key
                        .bytes()
                        .all(|b| b.is_ascii_uppercase() || b.is_ascii_digit() || b == b'_'),
                "malformed environment key"
            );
            assert!(seen.insert(key), "duplicate environment key");
            names.push(key);
        }
        members.insert(group, names);
    }
    let mut out =
        String::from("// @generated from src/molt/backend_environment.json by molt-ir/build.rs.\n");
    for (name, variants) in [
        (
            "BackendSemanticTarget",
            vec!["Native", "Wasm", "Both", "None"],
        ),
        ("BackendIdentityProjection", vec!["Raw", "Presence"]),
    ] {
        writeln!(out, "#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]\npub enum {name} {{ {} }}", variants.join(", ")).unwrap();
    }
    out.push_str("#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]\npub enum BackendEnvGroup {\n");
    for group in groups.keys() {
        writeln!(out, "{},", variant(group)).unwrap();
    }
    out.push_str("}\nimpl BackendEnvGroup {\n");
    for (field, method, result) in [
        ("request_scoped", "request_scoped", "bool"),
        ("native_worker", "native_codegen_identity", "bool"),
        ("diagnostic_bypass", "diagnostic_bypass", "bool"),
        ("request_reuse_only", "request_reuse_only", "bool"),
        (
            "semantic_target",
            "semantic_target",
            "BackendSemanticTarget",
        ),
        ("projection", "projection", "BackendIdentityProjection"),
    ] {
        writeln!(
            out,
            "pub const fn {method}(self) -> {result} {{ match self {{"
        )
        .unwrap();
        for (group, role) in groups {
            let value = if result == "bool" {
                role[field].as_bool().unwrap().to_string()
            } else {
                format!("{result}::{}", variant(role[field].as_str().unwrap()))
            };
            writeln!(out, "Self::{} => {value},", variant(group)).unwrap();
        }
        out.push_str("} }\n");
    }
    out.push_str("}\n#[allow(non_camel_case_types)]\n#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, serde::Serialize, serde::Deserialize)]\npub enum BackendEnv {\n");
    for names in members.values() {
        for name in names {
            writeln!(out, "{name},").unwrap();
        }
    }
    out.push_str("}\nimpl BackendEnv {\npub const ALL: &'static [Self] = &[\n");
    for names in members.values() {
        for name in names {
            writeln!(out, "Self::{name},").unwrap();
        }
    }
    out.push_str("];\npub const fn name(self) -> &'static str { match self {\n");
    for names in members.values() {
        for name in names {
            writeln!(out, "Self::{name} => \"{name}\",").unwrap();
        }
    }
    out.push_str("} }\npub const fn group(self) -> BackendEnvGroup { match self {\n");
    for (group, names) in &members {
        for name in names {
            writeln!(out, "Self::{name} => BackendEnvGroup::{},", variant(group)).unwrap();
        }
    }
    out.push_str("} }\npub fn from_name(name: &str) -> Option<Self> { match name {\n");
    for names in members.values() {
        for name in names {
            writeln!(out, "\"{name}\" => Some(Self::{name}),").unwrap();
        }
    }
    out.push_str("_ => None, } } }\n");
    let output = PathBuf::from(std::env::var_os("OUT_DIR").expect("Cargo output directory"));
    std::fs::write(output.join("backend_environment_keys.rs"), out)
        .expect("write environment keys");
}
