def test_generate_worker_produces_valid_js(tmp_path):
    from tools.generate_worker import generate_worker

    output = tmp_path / "worker.js"
    generate_worker(output, ["fs.bundle.read"], tmp_quota_mb=32)
    content = output.read_text()
    assert "fetch" in content
    assert "WebAssembly" in content


def test_generate_worker_contains_tmpfs(tmp_path):
    from tools.generate_worker import generate_worker

    output = tmp_path / "worker.js"
    generate_worker(output, ["fs.tmp.read", "fs.tmp.write"], tmp_quota_mb=64)
    content = output.read_text()
    assert "class TmpFs" in content
    assert "TMP_QUOTA_MB = 64" in content
    assert "ENOSPC: quota exceeded" in content


def test_generate_worker_contains_host_imports(tmp_path):
    from tools.generate_worker import generate_worker

    output = tmp_path / "worker.js"
    generate_worker(output, ["fs.bundle.read"], tmp_quota_mb=16)
    content = output.read_text()
    assert "createHostImports" in content
    assert "molt_vfs_read" in content
    assert "molt_vfs_write" in content
    assert "molt_log_host" in content


def test_generate_worker_contains_fetch_handler(tmp_path):
    from tools.generate_worker import generate_worker

    output = tmp_path / "worker.js"
    generate_worker(output, ["http.fetch"])
    content = output.read_text()
    assert "async fetch(request, env, ctx)" in content
    assert "export default" in content
    assert "molt_main" in content


def test_generate_worker_contains_wasi_shim(tmp_path):
    from tools.generate_worker import generate_worker

    output = tmp_path / "worker.js"
    generate_worker(output, [])
    content = output.read_text()
    assert "buildWasiShim" in content
    assert "wasi_snapshot_preview1" in content
    assert "fd_write" in content
    assert "fd_read" in content
    assert "proc_exit" in content
    assert "clock_time_get" in content


def test_generate_worker_capabilities_substituted(tmp_path):
    from tools.generate_worker import generate_worker

    output = tmp_path / "worker.js"
    generate_worker(output, ["fs.bundle.read", "http.fetch"], tmp_quota_mb=32)
    content = output.read_text()
    assert '"fs.bundle.read"' in content
    assert '"http.fetch"' in content
    assert "{{CAPABILITIES}}" not in content
    assert "{{TMP_QUOTA_MB}}" not in content
    assert "{{WASM_FILENAME}}" not in content


def test_generate_worker_custom_wasm_filename(tmp_path):
    from tools.generate_worker import generate_worker

    output = tmp_path / "worker.js"
    generate_worker(output, [], wasm_filename="custom.wasm")
    content = output.read_text()
    assert "custom.wasm" in content
    assert "worker_linked.wasm" not in content


def test_generate_worker_no_scaffold_warning(tmp_path):
    from tools.generate_worker import generate_worker

    output = tmp_path / "worker.js"
    generate_worker(output, [])
    content = output.read_text()
    assert "SCAFFOLD" not in content
    assert "NOT PRODUCTION READY" not in content


def test_generate_worker_stdio_capture(tmp_path):
    from tools.generate_worker import generate_worker

    output = tmp_path / "worker.js"
    generate_worker(output, [])
    content = output.read_text()
    assert "class StdioCapture" in content
    assert "writeStdout" in content
    assert "writeStderr" in content
    assert "readStdin" in content


def test_generate_split_worker_bootstrap_import_surface() -> None:
    from molt.cli import _generate_split_worker_js

    content = _generate_split_worker_js(
        shared_memory_initial_pages=8,
        shared_table_initial=16,
        shared_table_base=None,
    )

    assert 'import "./molt_vfs_browser.js";' in content
    assert 'import runtimeModule from "./molt_runtime.wasm";' in content
    assert 'import appModule from "./app.wasm";' in content
    assert "export default {" in content
    assert "async fetch(request, env, ctx)" in content


def test_generate_split_worker_contains_vfs_adapter_import() -> None:
    from molt.cli import _generate_split_worker_js

    content = _generate_split_worker_js(
        shared_memory_initial_pages=8,
        shared_table_initial=16,
        shared_table_base=None,
    )

    assert 'import "./molt_vfs_browser.js";' in content
    assert "new globalThis.MoltVfs()" in content


def test_generate_split_worker_contains_vfs_host_imports() -> None:
    from molt.cli import _generate_split_worker_js

    content = _generate_split_worker_js(
        shared_memory_initial_pages=8,
        shared_table_initial=16,
        shared_table_base=32,
    )

    assert "molt_vfs_read" in content
    assert "molt_vfs_write" in content
    assert "molt_vfs_exists" in content
    assert "molt_vfs_unlink" in content


def test_generate_split_worker_uses_worker_env_for_static_assets() -> None:
    from molt.cli import _generate_split_worker_js

    content = _generate_split_worker_js(
        shared_memory_initial_pages=8,
        shared_table_initial=16,
        shared_table_base=None,
    )

    assert "async fetch(request, env, ctx)" in content
    assert "env.__STATIC_CONTENT" in content


def test_generate_split_worker_defines_utf8_decoder_for_vfs_paths() -> None:
    from molt.cli import _generate_split_worker_js

    content = _generate_split_worker_js(
        shared_memory_initial_pages=8,
        shared_table_initial=16,
        shared_table_base=None,
    )

    assert "const utf8Decoder = new TextDecoder();" in content
    assert "utf8Decoder.decode" in content
    assert "UTF8_DECODER" not in content


def test_generate_split_worker_defines_wasi_vfs_errno_and_preopen_state() -> None:
    from molt.cli import _generate_split_worker_js

    content = _generate_split_worker_js(
        shared_memory_initial_pages=8,
        shared_table_initial=16,
        shared_table_base=None,
    )

    assert "const WASI_ERRNO_NOENT = 44;" in content
    assert "const WASI_ERRNO_NOSYS = 52;" in content
    assert "const WASI_OFLAGS_CREAT = 1;" in content
    assert "const wasiFiles = new Map();" in content
    assert "const wasiPreopens = [" in content
    assert "const preopenByFd = (fdNum) =>" in content
    assert "clock_res_get(id, outPtr)" in content
    assert "proc_raise() { return WASI_ERRNO_NOSYS; }" in content
    assert "fd_filestat_set_times: wasiUnsupported" in content


def test_generate_split_worker_replaces_path_stubs_with_vfs_backed_wasi_ops() -> None:
    from molt.cli import _generate_split_worker_js

    content = _generate_split_worker_js(
        shared_memory_initial_pages=8,
        shared_table_initial=16,
        shared_table_base=None,
    )

    assert "path_open() { return 44; }" not in content
    assert "path_filestat_get() { return 44; }" not in content
    assert "fd_prestat_get(fd, prestatPtr)" in content
    assert "fd_prestat_dir_name(fd, pathPtr, pathLen)" in content
    assert (
        "path_open(fd, _dirflags, pathPtr, pathLen, oflags, _rightsBase, _rightsInheriting, _fdflags, openedFdPtr)"
        in content
    )
    assert "path_filestat_get(fd, _flags, pathPtr, pathLen, bufPtr)" in content


def test_generate_split_wrangler_jsonc_limits_modules_to_deploy_surface() -> None:
    from molt.cli import _generate_split_wrangler_jsonc

    content = _generate_split_wrangler_jsonc("2026-04-11")

    assert '"main": "worker.js"' in content
    assert '"find_additional_modules": true' in content
    assert '"globs": ["worker.js", "molt_vfs_browser.js"]' in content
    assert '"globs": ["app.wasm", "molt_runtime.wasm"]' in content
    assert '"**/*.js"' not in content
    assert '"**/*.wasm"' not in content
    assert "output.wasm" not in content
    assert "output_linked.wasm" not in content


def test_generate_split_worker_installs_exported_table_refs() -> None:
    from molt.cli import _generate_split_worker_js

    content = _generate_split_worker_js(
        shared_memory_initial_pages=8,
        shared_table_initial=16,
        shared_table_base=32,
        app_table_ref_signatures={
            "__molt_table_ref_7": {"params": ["i64"], "result": "i64"}
        },
        runtime_table_ref_signatures={
            "__molt_table_ref_3": {"params": ["i32"], "result": "i32"}
        },
    )

    assert "const installTableRefs = (instance, table) => {" in content
    assert (
        "const ensureTableCapacityForExportedRefs = (instance, table) => {" in content
    )
    assert "installTableRefs(rtInstance, sharedTable);" in content
    assert "ensureTableCapacityForExportedRefs(appInstance, sharedTable);" in content
    assert "installTableRefs(appInstance, sharedTable);" in content
    assert "? [`MOLT_WASM_TABLE_BASE=${32}`]" in content


def test_generate_split_worker_uses_phased_call_indirect_routing() -> None:
    from molt.cli import _generate_split_worker_js

    content = _generate_split_worker_js(
        shared_memory_initial_pages=8,
        shared_table_initial=16,
        shared_table_base=32,
        app_table_ref_signatures={
            "__molt_table_ref_7": {"params": ["i64"], "result": "i64"}
        },
        runtime_table_ref_signatures={
            "__molt_table_ref_3": {"params": ["i32"], "result": "i32"}
        },
    )

    assert "const indirectName = `molt_call_indirect${arity}`;" in content
    assert "const idx = Number(fnIndex);" in content
    assert "const dispatchIdx = remapLegacyRuntimeSharedIdx(idx);" in content
    assert "const directName = `__molt_table_ref_${dispatchIdx}`;" in content
    assert "const indirectFn = appInstance?.exports?.[indirectName];" in content
    assert "return indirectFn(fnIndex, ...args);" in content
    assert "const tableFn = sharedTable.get(dispatchIdx);" in content
    assert 'if (typeof tableFn === "function") {' in content
    assert (
        "const signature = appTableRefSignatures[directName] || runtimeTableRefSignatures[directName] || null;"
        in content
    )
    assert "return callWithSignature(tableFn, signature, args);" in content
    assert "const rtDirectFn = rtInstance?.exports?.[directName];" in content
    assert (
        "return callWithSignature(rtDirectFn, runtimeTableRefSignatures[directName] || null, args);"
        in content
    )
    assert content.index(
        "const indirectFn = appInstance?.exports?.[indirectName];"
    ) < content.index("const tableFn = sharedTable.get(dispatchIdx);")
    assert content.index(
        "const tableFn = sharedTable.get(dispatchIdx);"
    ) < content.index("const rtDirectFn = rtInstance?.exports?.[directName];")
    assert "hasExportedTableRefs(appInstance)" not in content
    assert (
        "if (appInstance.exports.molt_table_init) appInstance.exports.molt_table_init();"
        in content
    )


def test_generate_split_worker_builds_runtime_import_wrappers_from_app_surface() -> (
    None
):
    from molt.cli import _generate_split_worker_js

    content = _generate_split_worker_js(
        shared_memory_initial_pages=8,
        shared_table_initial=16,
        shared_table_base=32,
        runtime_import_result_kinds={
            "function_set_builtin": "i64",
            "string_from_bytes": "i32",
        },
        runtime_import_signatures={
            "function_set_builtin": {"params": ["i64"], "result": "i64"},
            "string_from_bytes": {"params": ["i32", "i64", "i32"], "result": "i32"},
        },
        app_table_ref_signatures={
            "__molt_table_ref_1": {"params": ["i64"], "result": "i64"}
        },
        runtime_table_ref_signatures={
            "__molt_table_ref_2": {"params": ["i32"], "result": "i32"}
        },
    )

    assert "const buildRuntimeImports = (module, runtimeInstance) => {" in content
    assert "for (const entry of WebAssembly.Module.imports(module)) {" in content
    assert (
        'const runtimeImportResultKinds = {"function_set_builtin": "i64", "string_from_bytes": "i32"};'
        in content
    )
    assert (
        'const runtimeImportSignatures = {"function_set_builtin": {"params": ["i64"], "result": "i64"}, "string_from_bytes": {"params": ["i32", "i64", "i32"], "result": "i32"}};'
        in content
    )
    assert "const runtimeImportFallbacks =" in content
    assert '"fast_dict_get": {"call_arity": 2' in content
    assert '"strategy": "call_bind_ic"' in content
    assert '"dict_getitem": {"call_arity": null' in content
    assert '"exports": ["molt_dict_getitem_borrowed"]' in content
    assert (
        'const appTableRefSignatures = {"__molt_table_ref_1": {"params": ["i64"], "result": "i64"}};'
        in content
    )
    assert (
        'const runtimeTableRefSignatures = {"__molt_table_ref_2": {"params": ["i32"], "result": "i32"}};'
        in content
    )
    assert "const TAG_NONE = 0x0003000000000000n;" in content
    assert "const NONE_BITS = QNAN | TAG_NONE;" in content
    assert "const resultKind = runtimeImportResultKinds[entry.name] || null;" in content
    assert "const signature = runtimeImportSignatures[entry.name] || null;" in content
    assert "fn = runtimeFallback(entry.name);" in content
    assert 'entry.name === "fast_dict_get"' not in content
    assert (
        "? args.map((value, index) => normalizeValueForKind(value, signature.params[index] || null))"
        in content
    )
    assert "const callArgs = args.map((value, index) =>" in content
    assert "normalizeValueForKind(value, signature.params[index] || null)" in content
    assert "return normalizeImportResult(out, resultKind);" in content
    assert "const callWithSignature = (fn, signature, args) => {" in content
    assert "value === undefined || value === null" in content
    assert "? NONE_BITS" in content
    assert (
        "return normalizeI64Result(appInstance.exports.molt_isolate_import(...args));"
        in content
    )
    assert "molt_runtime: buildRuntimeImports(appModule, rtInstance)," in content
    assert "const runtimeAbiExports = (exports) => {" not in content


def test_effective_split_worker_table_base_uses_backend_authority() -> None:
    from molt.cli import _effective_split_worker_table_base

    assert (
        _effective_split_worker_table_base(
            wasm_table_base=4096,
            runtime_table_min=315,
            app_table_ref_signatures={
                "__molt_table_ref_4096": {"params": ["i64"], "result": "i64"},
                "__molt_table_ref_4189": {"params": ["i64"], "result": "i64"},
            },
        )
        == 4096
    )


def test_effective_split_worker_table_base_does_not_infer_fallback() -> None:
    from molt.cli import _effective_split_worker_table_base

    assert (
        _effective_split_worker_table_base(
            wasm_table_base=None,
            runtime_table_min=315,
            app_table_ref_signatures={
                "__molt_table_ref_4130": {"params": ["i64"], "result": "i64"},
            },
        )
        is None
    )


def test_effective_split_worker_table_base_rejects_export_mismatch() -> None:
    import pytest

    from molt.cli import _effective_split_worker_table_base

    with pytest.raises(ValueError, match="disagrees"):
        _effective_split_worker_table_base(
            wasm_table_base=4096,
            runtime_table_min=315,
            app_table_ref_signatures={
                "__molt_table_ref_4130": {"params": ["i64"], "result": "i64"},
            },
        )


def _wasm_vec(items: list[bytes]) -> bytes:
    import molt.wasm_artifact as wasm_artifact

    return wasm_artifact._write_wasm_varuint(len(items)) + b"".join(items)


def _wasm_function_type(params: list[int], results: list[int]) -> bytes:
    import molt.wasm_artifact as wasm_artifact

    return (
        b"\x60"
        + wasm_artifact._write_wasm_varuint(len(params))
        + bytes(params)
        + wasm_artifact._write_wasm_varuint(len(results))
        + bytes(results)
    )


def _wasm_function_import(module: str, name: str, type_index: int) -> bytes:
    import molt.wasm_artifact as wasm_artifact

    return (
        wasm_artifact._write_wasm_string(module)
        + wasm_artifact._write_wasm_string(name)
        + b"\x00"
        + wasm_artifact._write_wasm_varuint(type_index)
    )


def _wasm_function_export(name: str, function_index: int) -> bytes:
    import molt.wasm_artifact as wasm_artifact

    return (
        wasm_artifact._write_wasm_string(name)
        + b"\x00"
        + wasm_artifact._write_wasm_varuint(function_index)
    )


def _signature_fixture_wasm() -> bytes:
    import molt.wasm_artifact as wasm_artifact

    type_payload = _wasm_vec(
        [
            _wasm_function_type([0x7E], [0x7E]),
            _wasm_function_type([0x7F, 0x7E, 0x7F], [0x7F]),
            _wasm_function_type([], []),
        ]
    )
    import_payload = _wasm_vec(
        [
            _wasm_function_import("molt_runtime", "molt_function_set_builtin", 0),
            _wasm_function_import("molt_runtime", "molt_string_from_bytes", 1),
            _wasm_function_import("molt_runtime", "molt_resource_on_free", 2),
        ]
    )
    function_payload = _wasm_vec(
        [
            wasm_artifact._write_wasm_varuint(0),
            wasm_artifact._write_wasm_varuint(1),
        ]
    )
    export_payload = _wasm_vec(
        [
            _wasm_function_export("__molt_table_ref_7", 3),
            _wasm_function_export("__molt_table_ref_8", 4),
        ]
    )
    return wasm_artifact._build_wasm_sections(
        [
            (1, type_payload),
            (2, import_payload),
            (3, function_payload),
            (7, export_payload),
        ]
    )


def test_wasm_import_function_result_kinds_reads_wasm_bytes(tmp_path) -> None:
    from molt.wasm_artifact import _wasm_import_function_result_kinds

    wasm_path = tmp_path / "app.wasm"
    wasm_path.write_bytes(_signature_fixture_wasm())

    assert _wasm_import_function_result_kinds(
        wasm_path, module_name="molt_runtime"
    ) == {
        "molt_function_set_builtin": "i64",
        "molt_string_from_bytes": "i32",
        "molt_resource_on_free": "nil",
    }


def test_wasm_import_function_signatures_reads_wasm_bytes(tmp_path) -> None:
    from molt.wasm_artifact import _wasm_import_function_signatures

    wasm_path = tmp_path / "app.wasm"
    wasm_path.write_bytes(_signature_fixture_wasm())

    assert _wasm_import_function_signatures(wasm_path, module_name="molt_runtime") == {
        "molt_function_set_builtin": {"params": ["i64"], "result": "i64"},
        "molt_string_from_bytes": {"params": ["i32", "i64", "i32"], "result": "i32"},
        "molt_resource_on_free": {"params": [], "result": "nil"},
    }


def test_wasm_export_function_signatures_reads_wasm_bytes(tmp_path) -> None:
    from molt.wasm_artifact import _wasm_export_function_signatures

    wasm_path = tmp_path / "runtime.wasm"
    wasm_path.write_bytes(_signature_fixture_wasm())

    assert _wasm_export_function_signatures(
        wasm_path, export_name_prefix="__molt_table_ref_"
    ) == {
        "__molt_table_ref_7": {"params": ["i64"], "result": "i64"},
        "__molt_table_ref_8": {"params": ["i32", "i64", "i32"], "result": "i32"},
    }


def test_export_wasm_table_refs_adds_exports_for_active_slots(tmp_path) -> None:
    from molt.cli import _export_wasm_table_refs
    from molt.wasm_artifact import (
        _build_wasm_sections,
        _write_wasm_varuint,
        parse_wasm_exports,
    )

    type_payload = _write_wasm_varuint(1) + bytes([0x60, 0x00, 0x01, 0x7E])
    function_payload = _write_wasm_varuint(1) + _write_wasm_varuint(0)
    table_payload = _write_wasm_varuint(1) + bytes([0x70, 0x00, 0x04])
    element_payload = (
        _write_wasm_varuint(1)
        + bytes([0x00, 0x41, 0x03, 0x0B])
        + _write_wasm_varuint(1)
        + _write_wasm_varuint(0)
    )
    code_body = bytes([0x00, 0x42, 0x00, 0x0B])
    code_payload = (
        _write_wasm_varuint(1) + _write_wasm_varuint(len(code_body)) + code_body
    )
    wasm_bytes = _build_wasm_sections(
        [
            (1, type_payload),
            (3, function_payload),
            (4, table_payload),
            (9, element_payload),
            (10, code_payload),
        ]
    )
    wasm_path = tmp_path / "table_ref.wasm"
    wasm_path.write_bytes(wasm_bytes)

    _export_wasm_table_refs(wasm_path)

    exports = {
        export.name: (export.kind, export.index)
        for export in parse_wasm_exports(wasm_path.read_bytes())
    }
    assert exports["__molt_table_ref_3"] == (0, 0)
