use std::collections::BTreeSet;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use serde_json::Value as JsonValue;

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

#[test]
fn cargo_cdylib_selection_reports_runtime_wasm_with_fixed_abi_surface() {
    let root = workspace_root();
    let target_dir = root.join("target/wasm-cdylib-exports-test");
    let tmp_dir = root.join("tmp");
    fs::create_dir_all(&target_dir).expect("create target dir");
    fs::create_dir_all(&tmp_dir).expect("create tmp dir");
    let runtime_features = [
        "stdlib_micro",
        "builtin_set",
        "builtin_complex",
        "builtin_memoryview",
        "builtin_contextvars",
        "builtin_fcntl",
    ];

    let expected_cpython_abi = expected_cpython_abi_requested_exports();
    let cpython_abi_requested_exports = expected_cpython_abi
        .iter()
        .cloned()
        .collect::<Vec<_>>()
        .join("\n");
    let cpython_abi_requested_data_exports = expected_cpython_abi
        .iter()
        .filter(|name| {
            matches!(
                name.as_str(),
                "Py_EllipsisObject"
                    | "Py_GenericAliasType"
                    | "Py_None"
                    | "Py_NotImplementedSentinel"
                    | "Py_OptimizeFlag"
                    | "Py_Version"
                    | "PyExc_TypeError"
            )
        })
        .cloned()
        .collect::<Vec<_>>()
        .join("\n");
    let cpython_abi_export_flags = expected_cpython_abi
        .iter()
        .map(|name| format!("-C link-arg=--export-if-defined={name}"))
        .collect::<Vec<_>>();
    let mut rustflags = [
        "-C link-arg=--import-memory",
        "-C link-arg=--import-table",
        "-C link-arg=--growable-table",
        "-C link-arg=--export-dynamic",
        "-C target-feature=-reference-types,+simd128",
    ]
    .into_iter()
    .map(str::to_string)
    .collect::<Vec<_>>();
    rustflags.extend(cpython_abi_export_flags);
    let rustflags = rustflags.join(" ");
    let output = Command::new("cargo")
        .current_dir(&root)
        .env("CARGO_TARGET_DIR", &target_dir)
        .env("TMPDIR", &tmp_dir)
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
        .env("RUSTFLAGS", rustflags)
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
