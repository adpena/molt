use std::collections::BTreeSet;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

fn workspace_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(Path::parent)
        .expect("workspace root")
        .to_path_buf()
}

fn expected_fixed_exports() -> BTreeSet<String> {
    let manifest_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let source = fs::read_to_string(manifest_dir.join("src/wasm_abi_exports.rs"))
        .expect("read wasm_abi_exports.rs");
    let mut names = BTreeSet::from([
        "molt_runtime_shutdown".to_string(),
        "molt_set_wasm_table_base".to_string(),
    ]);
    for line in source.lines() {
        let trimmed = line.trim();
        if let Some(rest) = trimmed.strip_prefix("pub extern \"C\" fn ")
            && let Some((name, _)) = rest.split_once('(')
        {
            names.insert(name.trim().to_string());
        }
    }
    names
}

fn expected_variadic_shim_exports() -> BTreeSet<String> {
    BTreeSet::from([
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
    ])
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

#[test]
fn cargo_build_emits_runtime_wasm_with_fixed_abi_surface() {
    let root = workspace_root();
    let target_dir = root.join("target/wasm-cdylib-exports-test");
    let tmp_dir = root.join("tmp");
    fs::create_dir_all(&target_dir).expect("create target dir");
    fs::create_dir_all(&tmp_dir).expect("create tmp dir");

    let variadic_export_flags = expected_variadic_shim_exports()
        .iter()
        .map(|name| format!("-C link-arg=--export-if-defined={name}"))
        .collect::<Vec<_>>();
    let rustflags = [
        "-C link-arg=--import-memory",
        "-C link-arg=--import-table",
        "-C link-arg=--growable-table",
        "-C link-arg=--export-dynamic",
        "-C target-feature=-reference-types,+simd128",
    ]
    .into_iter()
    .chain(variadic_export_flags.iter().map(String::as_str))
    .join(" ");
    let output = Command::new("cargo")
        .current_dir(&root)
        .env("CARGO_TARGET_DIR", &target_dir)
        .env("TMPDIR", &tmp_dir)
        .env("MOLT_SESSION_ID", "test-wasm-cdylib-exports")
        .env("CARGO_INCREMENTAL", "0")
        .env("RUSTFLAGS", rustflags)
        .args([
            "build",
            "--package",
            "molt-runtime",
            "--profile",
            "dev-fast",
            "--target",
            "wasm32-wasip1",
            "--no-default-features",
            "--features",
            "stdlib_micro,builtin_set,builtin_complex,builtin_memoryview,builtin_contextvars,builtin_fcntl",
        ])
        .output()
        .expect("run cargo build for wasm runtime");

    assert!(
        output.status.success(),
        "cargo build failed\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr),
    );

    let runtime_wasm = target_dir.join("wasm32-wasip1/dev-fast/molt_runtime.wasm");
    assert!(
        runtime_wasm.exists(),
        "cargo build did not emit stable runtime wasm artifact at {}",
        runtime_wasm.display()
    );
    let export_names = read_export_names(&runtime_wasm);
    let expected = expected_fixed_exports();
    let missing: Vec<String> = expected.difference(&export_names).cloned().collect();
    assert!(
        missing.is_empty(),
        "missing fixed wasm cdylib exports: {missing:?}"
    );
    let expected_variadic = expected_variadic_shim_exports();
    let missing_variadic: Vec<String> = expected_variadic
        .difference(&export_names)
        .cloned()
        .collect();
    assert!(
        missing_variadic.is_empty(),
        "missing variadic shim wasm exports: {missing_variadic:?}"
    );
}
