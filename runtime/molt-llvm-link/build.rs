use serde::Deserialize;
use std::{
    collections::HashSet,
    env, fs,
    path::{Path, PathBuf},
};

const PLAN_ENV: &str = "MOLT_LLVM_LINK_PLAN";

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct LinkPlan {
    schema: String,
    target_os: String,
    target_arches: Vec<String>,
    host_target: String,
    host_abi: HostAbi,
    llvm_config: PathBuf,
    llvm_version: String,
    driver: PathBuf,
    libraries: Vec<Library>,
    policy: Option<serde_json::Value>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct HostAbi {
    architecture: String,
    pointer_bits: u16,
    header_bits: u16,
    byte_order: String,
}

#[derive(Clone, Copy, Deserialize)]
#[serde(rename_all = "lowercase")]
enum LinkKind {
    Static,
    Dylib,
    Framework,
}

#[derive(Clone, Copy, Deserialize)]
#[serde(rename_all = "lowercase")]
enum Origin {
    Sdk,
    System,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Library {
    kind: LinkKind,
    name: String,
    path: PathBuf,
    source: Origin,
    policy_admitted: Option<bool>,
}

fn selected_file(path: &Path) {
    let text = path.to_str().expect("LLVM input path is not UTF-8");
    assert!(
        path.is_absolute() && !text.contains(['\r', '\n', '\0']),
        "LLVM input must name an absolute regular file: {text}"
    );
    assert!(path.is_file(), "selected LLVM input is absent: {text}");
    println!("cargo:rerun-if-changed={text}");
}

fn main() {
    let output = PathBuf::from(env::var_os("OUT_DIR").unwrap()).join("native_links.rs");
    if env::var_os("CARGO_FEATURE_NATIVE_LINK").is_none() {
        fs::write(output, "").expect("cannot publish inactive native metadata");
        return;
    }
    println!("cargo:rerun-if-env-changed={PLAN_ENV}");
    println!("cargo:rerun-if-env-changed=LLVM_CONFIG_PATH");
    let raw = env::var(PLAN_ENV).expect(
        "LLVM requires the admitted native link plan; run through molt's toolchain environment",
    );
    let plan: LinkPlan = serde_json::from_str(&raw).expect("invalid native LLVM link plan");
    assert_eq!(
        plan.schema, "molt.llvm-native-link.v1",
        "unknown LLVM plan schema"
    );
    assert_eq!(
        plan.target_os,
        env::var("CARGO_CFG_TARGET_OS").unwrap(),
        "LLVM SDK target OS differs"
    );
    assert!(
        plan.target_arches
            .contains(&env::var("CARGO_CFG_TARGET_ARCH").unwrap()),
        "LLVM SDK target architecture differs"
    );
    assert_eq!(
        plan.host_abi.pointer_bits.to_string(),
        env::var("CARGO_CFG_TARGET_POINTER_WIDTH").unwrap(),
        "LLVM SDK pointer ABI differs"
    );
    assert_eq!(
        plan.host_abi.byte_order,
        env::var("CARGO_CFG_TARGET_ENDIAN").unwrap(),
        "LLVM SDK byte order differs"
    );
    assert!(
        !plan.host_target.is_empty()
            && !plan.host_abi.architecture.is_empty()
            && matches!(plan.host_abi.header_bits, 32 | 64),
        "invalid LLVM SDK host ABI"
    );
    assert_eq!(
        Some(plan.llvm_config.as_os_str()),
        env::var_os("LLVM_CONFIG_PATH").as_deref(),
        "LLVM configuration selection differs from admitted plan"
    );
    assert!(
        !plan.llvm_version.is_empty() && !plan.libraries.is_empty(),
        "empty native LLVM plan"
    );
    selected_file(&plan.llvm_config);
    selected_file(&plan.driver);
    let mut generated = String::new();
    let mut sdk_count = 0;
    let mut selected = HashSet::new();
    for library in &plan.libraries {
        selected_file(&library.path);
        assert!(
            selected.insert(&library.path),
            "duplicate native LLVM library operand"
        );
        assert!(
            !library.name.is_empty()
                && library
                    .name
                    .bytes()
                    .all(|c| c.is_ascii_alphanumeric() || b"_+.-".contains(&c)),
            "invalid LLVM ABI library name"
        );
        assert_eq!(
            library.policy_admitted.is_some(),
            plan.policy.is_some(),
            "LLVM dependency capability projection differs"
        );
        if matches!(library.source, Origin::Sdk) {
            assert!(
                matches!(library.kind, LinkKind::Static),
                "LLVM SDK input is not static"
            );
            sdk_count += 1;
        }
        let (name, kind, modifiers) = match library.kind {
            // Bundle once in this dependency rlib. Absolute +verbatim selection
            // prevents another Cargo -L root from substituting same-name bytes.
            LinkKind::Static => (
                library.path.to_str().unwrap(),
                "static",
                ", modifiers = \"+bundle,+verbatim\"",
            ),
            // These are intentional target ABI selectors. The selected SDK file
            // is an observation; the actual emitted ELF/Mach-O audit decides
            // whether the final program has an admissible shared closure.
            LinkKind::Dylib => (library.name.as_str(), "dylib", ""),
            LinkKind::Framework => (library.name.as_str(), "framework", ""),
        };
        let literal = format!("{name:?}");
        generated.push_str(&format!(
            "#[link(name = {literal}, kind = \"{kind}\"{modifiers})]\nunsafe extern \"C\" {{}}\n"
        ));
    }
    assert!(sdk_count > 0, "LLVM link plan has no static SDK archives");
    fs::write(output, generated).expect("cannot publish native LLVM link metadata");
}
