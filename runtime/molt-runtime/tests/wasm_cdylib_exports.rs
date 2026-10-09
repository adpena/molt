use std::collections::BTreeSet;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use serde_json::Value as JsonValue;

#[path = "../../build_support/wasi_sysroot.rs"]
mod wasi_c_abi;

fn workspace_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(Path::parent)
        .expect("workspace root")
        .to_path_buf()
}

fn expected_fixed_exports(enabled_features: &[&str]) -> BTreeSet<String> {
    let manifest_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let source = fs::read_to_string(manifest_dir.join("src/wasm_abi_exports.rs"))
        .expect("read wasm_abi_exports.rs");
    let enabled_features = enabled_features.iter().copied().collect::<BTreeSet<_>>();
    let mut names = BTreeSet::from([
        "molt_runtime_shutdown".to_string(),
        "molt_set_wasm_table_base".to_string(),
    ]);
    let mut gated_feature: Option<String> = None;
    for line in source.lines() {
        let trimmed = line.trim();
        if let Some(feature) = trimmed
            .strip_prefix("#[cfg(feature = \"")
            .and_then(|rest| rest.strip_suffix("\")]"))
        {
            gated_feature = Some(feature.to_string());
            continue;
        }
        if let Some(rest) = trimmed.strip_prefix("pub extern \"C\" fn ")
            && let Some((name, _)) = rest.split_once('(')
        {
            if gated_feature
                .as_deref()
                .is_some_and(|feature| !enabled_features.contains(feature))
            {
                gated_feature = None;
                continue;
            }
            names.insert(name.trim().to_string());
            gated_feature = None;
        } else if !trimmed.is_empty() && !trimmed.starts_with("#[") {
            gated_feature = None;
        }
    }
    names
}

fn expected_cpython_abi_requested_exports() -> BTreeSet<String> {
    let mut names = BTreeSet::from([
        "PyArg_ParseTuple".to_string(),
        "PyArg_ParseTupleAndKeywords".to_string(),
        "PyArg_UnpackTuple".to_string(),
        "PyArg_VaParseTupleAndKeywords".to_string(),
        "PyTuple_Pack".to_string(),
        "PyObject_CallFunction".to_string(),
        "PyObject_CallFunctionObjArgs".to_string(),
        "PyObject_CallMethod".to_string(),
        "PyObject_CallMethodObjArgs".to_string(),
        "Py_BuildValue".to_string(),
        "_Py_BuildValue_SizeT".to_string(),
        "Py_VaBuildValue".to_string(),
        "PyUnicode_FromFormat".to_string(),
        "PyUnicode_FromFormatV".to_string(),
        "PyOS_snprintf".to_string(),
        "PyOS_vsnprintf".to_string(),
        "PyOS_string_to_double".to_string(),
        "PyOS_strtol".to_string(),
        "PyOS_strtoul".to_string(),
        "PyErr_WarnFormat".to_string(),
        "PyErr_Format".to_string(),
        "PyErr_FormatV".to_string(),
        "PyErr_FormatUnraisable".to_string(),
        "PySys_WriteStderr".to_string(),
    ]);
    names.extend(
        [
            "PyObject_Init",
            "PyObject_InitVar",
            "PyModuleDef_Init",
            "PyType_Ready",
            "_PyObject_New",
            "PyMemoryView_FromMemory",
            "Py_None",
            "Py_EllipsisObject",
            "Py_GenericAliasType",
            "PyRange_Type",
            "Py_NotImplementedSentinel",
            "Py_OptimizeFlag",
            "Py_Version",
            "PyFloat_Check",
            "PyExc_TypeError",
        ]
        .into_iter()
        .map(str::to_string),
    );
    names
}

fn requested_data_exports(exports: &BTreeSet<String>) -> Vec<String> {
    let manifest: JsonValue =
        serde_json::from_str(include_str!("../../../wasm/wasm_abi_generated.json"))
            .expect("generated WASM ABI manifest");
    let kinds = manifest["external_native_link_imports"]["symbol_kinds"]
        .as_object()
        .expect("generated external ABI symbol kinds");
    exports
        .iter()
        .filter_map(|name| match kinds.get(name).and_then(JsonValue::as_str) {
            Some("data") => Some(name.clone()),
            Some("function") => None,
            kind => panic!("requested ABI export {name} lacks a canonical symbol kind: {kind:?}"),
        })
        .collect()
}

#[test]
fn requested_data_roots_use_canonical_abi_symbol_kinds() {
    let exports = expected_cpython_abi_requested_exports();
    let data = requested_data_exports(&exports);
    assert!(data.iter().any(|name| name == "PyRange_Type"));
    assert!(data.iter().any(|name| name == "PyExc_TypeError"));
    assert!(!data.iter().any(|name| name == "PyObject_Init"));
}

fn read_export_names(path: &Path) -> BTreeSet<String> {
    let data = fs::read(path).expect("read wasm artifact");
    assert!(
        data.starts_with(b"\0asm"),
        "expected wasm magic in {path:?}"
    );
    let mut offset = 8usize;
    while offset < data.len() {
        let section_id = data[offset];
        offset += 1;
        let (section_len, next) = read_varuint(&data, offset);
        offset = next;
        let end = offset + section_len;
        if section_id == 7 {
            let (count, mut cursor) = read_varuint(&data, offset);
            let mut names = BTreeSet::new();
            for _ in 0..count {
                let (name_len, name_cursor) = read_varuint(&data, cursor);
                cursor = name_cursor;
                let name_end = cursor + name_len;
                let name = std::str::from_utf8(&data[cursor..name_end])
                    .expect("utf-8 export name")
                    .to_string();
                cursor = name_end + 1;
                let (_, index_cursor) = read_varuint(&data, cursor);
                cursor = index_cursor;
                names.insert(name);
            }
            return names;
        }
        offset = end;
    }
    panic!("missing export section in {path:?}");
}

fn read_varuint(data: &[u8], mut offset: usize) -> (usize, usize) {
    let mut value = 0usize;
    let mut shift = 0usize;
    loop {
        let byte = data[offset];
        offset += 1;
        value |= usize::from(byte & 0x7f) << shift;
        if byte & 0x80 == 0 {
            return (value, offset);
        }
        shift += 7;
    }
}

fn reported_runtime_cdylib(stdout: &str, target_dir: &Path) -> PathBuf {
    let target_dir = fs::canonicalize(target_dir).expect("canonical target dir");
    let mut reported = BTreeSet::new();
    for line in stdout.lines() {
        let Ok(message) = serde_json::from_str::<JsonValue>(line) else {
            continue;
        };
        if message.get("reason").and_then(JsonValue::as_str) != Some("compiler-artifact") {
            continue;
        }
        let package_id = message
            .get("package_id")
            .and_then(JsonValue::as_str)
            .unwrap_or_default();
        let target = message.get("target").and_then(JsonValue::as_object);
        let target_name = target
            .and_then(|target| target.get("name"))
            .and_then(JsonValue::as_str);
        let crate_types = target
            .and_then(|target| target.get("crate_types"))
            .and_then(JsonValue::as_array);
        if !package_id.contains("molt-runtime")
            || target_name != Some("molt_runtime")
            || !crate_types.is_some_and(|crate_types| {
                crate_types
                    .iter()
                    .any(|crate_type| crate_type.as_str() == Some("cdylib"))
            })
        {
            continue;
        }
        let Some(filenames) = message.get("filenames").and_then(JsonValue::as_array) else {
            continue;
        };
        for filename in filenames {
            let Some(filename) = filename.as_str() else {
                continue;
            };
            let path = PathBuf::from(filename);
            if path.extension().and_then(|extension| extension.to_str()) != Some("wasm") {
                continue;
            }
            let path = fs::canonicalize(&path)
                .unwrap_or_else(|error| panic!("canonicalize Cargo artifact {path:?}: {error}"));
            assert!(
                path.starts_with(&target_dir),
                "Cargo reported runtime cdylib outside target dir: {}",
                path.display()
            );
            reported.insert(path);
        }
    }
    assert_eq!(
        reported.len(),
        1,
        "Cargo must report exactly one runtime cdylib artifact, got {reported:?}"
    );
    reported.into_iter().next().expect("one reported cdylib")
}

// Keep effective Cargo flags as argument tokens throughout construction. SDK
// paths can contain spaces; only Cargo's unit separator encodes this vector.
fn wasi_cargo_rustflags(plan: &wasi_c_abi::WasiCAbiPlan) -> Vec<String> {
    let mut flags = Vec::new();
    for directory in plan.native_search_directories() {
        flags.push("-L".to_owned());
        flags.push(format!(
            "native={}",
            directory.to_str().expect("validated UTF-8 SDK path")
        ));
    }
    flags.extend(
        [
            "-C",
            "link-self-contained=no",
            "-C",
            "linker-flavor=wasm-ld",
        ]
        .into_iter()
        .map(str::to_owned),
    );
    flags
}

#[test]
fn cargo_cdylib_selection_reports_runtime_wasm_with_fixed_abi_surface() {
    let root = workspace_root();
    let projection = std::env::var(wasi_c_abi::PLAN_ENV)
        .expect("project-owned complete WASI C ABI plan is required before nested Cargo");
    let plan =
        wasi_c_abi::WasiCAbiPlan::decode(&projection).expect("selected WASI C ABI projection");
    let target_dir = PathBuf::from(
        std::env::var_os("CARGO_TARGET_DIR").expect("selected Cargo target directory is required"),
    )
    .join("wasm-cdylib-exports-test");
    fs::create_dir_all(&target_dir).expect("create selected target dir");
    let runtime_features = [
        "stdlib_micro",
        "builtin_set",
        "builtin_complex",
        "builtin_memoryview",
        "builtin_fcntl",
    ];

    let expected_cpython_abi = expected_cpython_abi_requested_exports();
    let cpython_abi_requested_exports = expected_cpython_abi
        .iter()
        .cloned()
        .collect::<Vec<_>>()
        .join("\n");
    let cpython_abi_requested_data_exports =
        requested_data_exports(&expected_cpython_abi).join("\n");
    let mut rustflags = wasi_cargo_rustflags(&plan);
    rustflags.extend(
        [
            "-C",
            "link-arg=--import-memory",
            "-C",
            "link-arg=--import-table",
            "-C",
            "link-arg=--growable-table",
            "-C",
            "link-arg=--export-dynamic",
            "-C",
            "target-feature=-reference-types,+simd128",
        ]
        .into_iter()
        .map(str::to_owned),
    );
    for name in &expected_cpython_abi {
        rustflags.extend([
            "-C".to_owned(),
            format!("link-arg=--export-if-defined={name}"),
        ]);
    }
    let encoded_flags = rustflags.join("\x1f");
    let output = Command::new("cargo")
        .current_dir(&root)
        .env("CARGO_TARGET_DIR", &target_dir)
        .env("MOLT_SESSION_ID", "test-wasm-cdylib-exports")
        .env(
            "MOLT_WASM_CPYTHON_ABI_EXPORTS",
            cpython_abi_requested_exports,
        )
        .env(
            "MOLT_WASM_CPYTHON_ABI_DATA_EXPORTS",
            cpython_abi_requested_data_exports,
        )
        .env("CARGO_INCREMENTAL", "0")
        .env_remove("RUSTFLAGS")
        .env("CARGO_ENCODED_RUSTFLAGS", encoded_flags)
        .env("CARGO_TARGET_WASM32_WASIP1_LINKER", &plan.linker)
        .args([
            "rustc",
            "--package",
            "molt-runtime",
            "--profile",
            "dev-fast",
            "--target",
            "wasm32-wasip1",
            "--lib",
            "--no-default-features",
            "--features",
            &runtime_features.join(","),
            "--crate-type",
            "cdylib",
            "--message-format=json-render-diagnostics",
        ])
        .output()
        .expect("run cargo build for wasm runtime");

    assert!(
        output.status.success(),
        "cargo build failed\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr),
    );

    let runtime_wasm =
        reported_runtime_cdylib(&String::from_utf8_lossy(&output.stdout), &target_dir);
    let export_names = read_export_names(&runtime_wasm);
    let expected = expected_fixed_exports(&runtime_features);
    let missing: Vec<String> = expected.difference(&export_names).cloned().collect();
    assert!(
        missing.is_empty(),
        "missing fixed wasm cdylib exports: {missing:?}"
    );
    let missing_cpython_abi: Vec<String> = expected_cpython_abi
        .difference(&export_names)
        .cloned()
        .collect();
    assert!(
        missing_cpython_abi.is_empty(),
        "missing requested CPython ABI wasm exports: {missing_cpython_abi:?}"
    );
}

#[test]
fn wasi_c_abi_wire_refuses_obsolete_and_non_ascii_coordinates() {
    fn encode(fields: &[String]) -> String {
        fields
            .join("\0")
            .bytes()
            .map(|byte| format!("{byte:02x}"))
            .collect()
    }
    // Independently authored legal language coordinates, not producer output.
    let mut fields = vec![
        "molt.wasi-c-abi.v2",
        "wasm32-wasip1",
        "single",
        "34.0",
        "23.0.0",
        "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
        "/sdk",
        "/sdk/share/wasi-sysroot",
        "/sdk/share/wasi-sysroot/include/wasm32-wasip1",
        "/sdk/bin/clang",
        "/sdk/bin/wasm-ld",
    ]
    .into_iter()
    .map(str::to_owned)
    .collect::<Vec<_>>();
    for role in [
        "libc",
        "long_double",
        "compiler_rt",
        "crt_command",
        "crt_reactor",
    ] {
        fields.extend([
            role.to_owned(),
            format!("/sdk/{role}"),
            "0".into(),
            "a".repeat(64),
        ]);
    }
    for (index, value) in [
        (0, "molt.wasi-c-abi.v0"),
        (1, "wasm32-wasip2"),
        (2, "threads"),
        (3, "３４.0"),
        (4, "23.０.0"),
    ] {
        let mut invalid = fields.clone();
        invalid[index] = value.into();
        assert!(wasi_c_abi::WasiCAbiPlan::decode(&encode(&invalid)).is_err());
    }
    assert!(wasi_c_abi::WasiCAbiPlan::decode(&"a".repeat(32_001)).is_err());
    assert!(wasi_c_abi::WasiCAbiPlan::decode("AA").is_err());
}

#[test]
fn wasi_c_abi_wire_admits_native_paths_and_rejects_member_drift() {
    let nonce = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("clock")
        .as_nanos();
    let sdk = std::env::temp_dir().join(format!("molt c abi wire {}-{nonce}", std::process::id()));
    std::fs::create_dir(&sdk).expect("exclusive fixture root");
    let root = sdk.join("sysroot");
    let include = root.join("include");
    let native_lib = root.join("lib/wasm32-wasip1");
    let compiler_rt_lib = sdk.join("lib/clang/23/lib/wasi");
    let driver = sdk.join("clang");
    let linker = sdk.join("wasm-ld");
    std::fs::create_dir_all(&include).expect("fixture headers");
    std::fs::create_dir_all(&native_lib).expect("fixture libc directory");
    std::fs::create_dir_all(&compiler_rt_lib).expect("fixture compiler-rt directory");
    std::fs::write(&driver, b"driver fixture, never executed").expect("fixture driver");
    let mut fields: Vec<String> = [
        "molt.wasi-c-abi.v2",
        "wasm32-wasip1",
        "single",
        "34.0",
        "23.0.0",
    ]
    .into_iter()
    .map(str::to_owned)
    .collect();
    fields.push("a".repeat(64));
    fields.extend(
        [&sdk, &root, &include, &driver, &linker]
            .map(|path| path.to_str().expect("native UTF-8 path").to_owned()),
    );
    for role in [
        "libc",
        "long_double",
        "compiler_rt",
        "crt_command",
        "crt_reactor",
    ] {
        let directory = if role == "compiler_rt" {
            &compiler_rt_lib
        } else {
            &native_lib
        };
        let path = directory.join(role);
        std::fs::write(&path, b"fixture").expect("fixture member");
        fields.extend([
            role.to_owned(),
            path.to_str().expect("path").to_owned(),
            "7".to_owned(),
            "a".repeat(64),
        ]);
    }
    let encode = |input: &[String]| {
        input
            .join("\0")
            .bytes()
            .map(|byte| format!("{byte:02x}"))
            .collect::<String>()
    };
    // A declared hash is structurally checked here, not authenticated by Rust.
    let admitted =
        wasi_c_abi::WasiCAbiPlan::decode(&encode(&fields)).expect("independent legal wire");
    assert_eq!(
        admitted.members,
        vec![
            ("libc".to_owned(), native_lib.join("libc")),
            ("long_double".to_owned(), native_lib.join("long_double")),
            (
                "compiler_rt".to_owned(),
                compiler_rt_lib.join("compiler_rt")
            ),
            ("crt_command".to_owned(), native_lib.join("crt_command")),
            ("crt_reactor".to_owned(), native_lib.join("crt_reactor")),
        ]
    );
    // An otherwise valid generation must not admit a truncated final byte.
    assert!(wasi_c_abi::WasiCAbiPlan::decode(&format!("{}0", encode(&fields))).is_err());
    assert_eq!(admitted.driver, driver);
    // Independent literal order: the four sysroot members share one directory,
    // while compiler-rt owns a second. The projection preserves first occurrence.
    assert_eq!(
        admitted.native_search_directories(),
        vec![native_lib.as_path(), compiler_rt_lib.as_path()]
    );
    let expected = vec![
        "-L".to_owned(),
        format!("native={}", native_lib.to_str().expect("libc path")),
        "-L".to_owned(),
        format!(
            "native={}",
            compiler_rt_lib.to_str().expect("compiler-rt path")
        ),
        "-C".to_owned(),
        "link-self-contained=no".to_owned(),
        "-C".to_owned(),
        "linker-flavor=wasm-ld".to_owned(),
    ];
    let flags = wasi_cargo_rustflags(&admitted);
    assert_eq!(flags, expected);
    let encoded = flags.join("\x1f");
    assert_eq!(
        encoded.split('\x1f').collect::<Vec<_>>(),
        expected.iter().map(String::as_str).collect::<Vec<_>>()
    );
    let missing_search = expected[4..].to_vec();
    let missing_compiler_rt = [expected[..2].to_vec(), expected[4..].to_vec()].concat();
    let reversed_search = [
        expected[2..4].to_vec(),
        expected[..2].to_vec(),
        expected[4..].to_vec(),
    ]
    .concat();
    // Reproduce the retired encoder as an independent rejected mutation.
    let whitespace_split = expected
        .join(" ")
        .split_whitespace()
        .map(str::to_owned)
        .collect::<Vec<_>>();
    let mut invalid_flags = vec![
        missing_search,
        missing_compiler_rt,
        reversed_search,
        whitespace_split,
    ];
    for kind in ["native", "all"] {
        let mut foreign_first = vec![
            "-L".to_owned(),
            format!(
                "{kind}={}",
                sdk.join("foreign").to_str().expect("foreign path")
            ),
        ];
        foreign_first.extend(expected.iter().cloned());
        invalid_flags.push(foreign_first);
    }
    for target in ["wasm32-wasip1", "wasm32-unknown-unknown"] {
        admitted
            .validate_cargo_mode(target, linker.to_str().expect("linker"), &encoded)
            .expect("complete SDK-first target context");
        for invalid in &invalid_flags {
            assert!(
                admitted
                    .validate_cargo_mode(
                        target,
                        linker.to_str().expect("linker"),
                        &invalid.join("\x1f")
                    )
                    .is_err(),
                "invalid SDK search context accepted: {invalid:?}"
            );
        }
    }
    for (index, value) in [
        (11, "compiler_rt"),
        (13, "01"),
        (13, "true"),
        (13, "8589934593"),
    ] {
        let mut invalid = fields.clone();
        invalid[index] = value.into();
        assert!(wasi_c_abi::WasiCAbiPlan::decode(&encode(&invalid)).is_err());
    }
    #[cfg(unix)]
    {
        let mut invalid = fields.clone();
        invalid[6] = format!("/{}", invalid[6]);
        assert!(wasi_c_abi::WasiCAbiPlan::decode(&encode(&invalid)).is_err());
    }
    std::fs::write(native_lib.join("libc"), b"changed extent").expect("mutate owned fixture");
    assert!(wasi_c_abi::WasiCAbiPlan::decode(&encode(&fields)).is_err());
    std::fs::remove_dir_all(sdk).expect("remove owned fixture");
}
