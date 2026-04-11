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
    assert "path_open(fd, _dirflags, pathPtr, pathLen, oflags, _rightsBase, _rightsInheriting, _fdflags, openedFdPtr)" in content
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
        app_table_ref_signatures={"__molt_table_ref_7": {"params": ["i64"], "result": "i64"}},
        runtime_table_ref_signatures={"__molt_table_ref_3": {"params": ["i32"], "result": "i32"}},
    )

    assert "const installTableRefs = (instance, table) => {" in content
    assert "const ensureTableCapacityForExportedRefs = (instance, table) => {" in content
    assert "installTableRefs(rtInstance, sharedTable);" in content
    assert "ensureTableCapacityForExportedRefs(appInstance, sharedTable);" in content
    assert "installTableRefs(appInstance, sharedTable);" in content


def test_generate_split_worker_prefers_live_shared_table_for_call_indirect() -> None:
    from molt.cli import _generate_split_worker_js

    content = _generate_split_worker_js(
        shared_memory_initial_pages=8,
        shared_table_initial=16,
        shared_table_base=32,
    )

    assert "const indirectName = `molt_call_indirect${arity}`;" in content
    assert "const idx = Number(fnIndex);" in content
    assert "const dispatchIdx = remapLegacyRuntimeSharedIdx(idx);" in content
    assert "const tableFn = sharedTable.get(dispatchIdx);" in content
    assert 'if (typeof tableFn === "function") {' in content
    assert "return tableFn(...args);" in content
    assert "const indirectFn = appInstance?.exports?.[indirectName];" in content
    assert "return indirectFn(fnIndex, ...args);" in content
    assert "const directName = `__molt_table_ref_${dispatchIdx}`;" in content
    assert "const rtDirectFn = rtInstance?.exports?.[directName];" in content
    assert "return rtDirectFn(...args);" in content


def test_generate_split_worker_remaps_legacy_reserved_runtime_slots() -> None:
    from molt.cli import _generate_split_worker_js

    content = _generate_split_worker_js(
        shared_memory_initial_pages=8,
        shared_table_initial=16,
        shared_table_base=4130,
        app_table_ref_signatures={"__molt_table_ref_4189": {"params": ["i64"], "result": "i64"}},
        runtime_table_ref_signatures={"__molt_table_ref_315": {"params": ["i32"], "result": "i32"}},
    )

    assert "const LEGACY_WASM_TABLE_BASE = 256;" in content
    assert "const RESERVED_RUNTIME_CALLABLE_BASE = 33;" in content
    assert "const RESERVED_RUNTIME_SHARED_PREFIX_LEN = 77;" in content
    assert "const dispatchIdx = remapLegacyRuntimeSharedIdx(idx);" in content
    assert "idx >= LEGACY_WASM_TABLE_BASE + RESERVED_RUNTIME_CALLABLE_BASE" in content
    assert "idx < LEGACY_WASM_TABLE_BASE + RESERVED_RUNTIME_SHARED_PREFIX_LEN" in content
    assert "return idx - LEGACY_WASM_TABLE_BASE + 4130;" in content
    assert "const directName = `__molt_table_ref_${dispatchIdx}`;" in content
    assert "const tableFn = sharedTable.get(dispatchIdx);" in content


def test_generate_split_worker_builds_runtime_import_wrappers_from_app_surface() -> None:
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
        app_table_ref_signatures={"__molt_table_ref_1": {"params": ["i64"], "result": "i64"}},
        runtime_table_ref_signatures={"__molt_table_ref_2": {"params": ["i32"], "result": "i32"}},
    )

    assert "const buildRuntimeImports = (module, runtimeInstance) => {" in content
    assert "for (const entry of WebAssembly.Module.imports(module)) {" in content
    assert 'const runtimeImportResultKinds = {"function_set_builtin": "i64", "string_from_bytes": "i32"};' in content
    assert 'const runtimeImportSignatures = {"function_set_builtin": {"params": ["i64"], "result": "i64"}, "string_from_bytes": {"params": ["i32", "i64", "i32"], "result": "i32"}};' in content
    assert 'const appTableRefSignatures = {"__molt_table_ref_1": {"params": ["i64"], "result": "i64"}};' in content
    assert 'const runtimeTableRefSignatures = {"__molt_table_ref_2": {"params": ["i32"], "result": "i32"}};' in content
    assert 'const resultKind = runtimeImportResultKinds[entry.name] || null;' in content
    assert "const signature = runtimeImportSignatures[entry.name] || null;" in content
    assert "? args.map((value, index) => normalizeValueForKind(value, signature.params[index] || null))" in content
    assert "return normalizeImportResult(out, resultKind);" in content
    assert "const callWithSignature = (fn, signature, args) => {" in content
    assert "return normalizeI64Result(appInstance.exports.molt_isolate_import(...args));" in content
    assert "molt_runtime: buildRuntimeImports(appModule, rtInstance)," in content
    assert "const runtimeAbiExports = (exports) => {" not in content


def test_effective_split_worker_table_base_prefers_app_inferred_base() -> None:
    from molt.cli import _effective_split_worker_table_base

    assert (
        _effective_split_worker_table_base(
            wasm_table_base=None,
            runtime_table_min=315,
            app_table_ref_signatures={
                "__molt_table_ref_4130": {"params": ["i64"], "result": "i64"},
                "__molt_table_ref_4189": {"params": ["i64"], "result": "i64"},
            },
        )
        == 4130
    )


def test_wasm_import_function_result_kinds_parses_objdump_output(
    monkeypatch, tmp_path
) -> None:
    import subprocess

    from molt.cli import _wasm_import_function_result_kinds

    wasm_path = tmp_path / "app.wasm"
    wasm_path.write_bytes(b"\x00asm\x01\x00\x00\x00")

    monkeypatch.setattr("molt.cli.shutil.which", lambda name: "/opt/homebrew/bin/wasm-objdump")

    def fake_run(*args, **kwargs):
        return subprocess.CompletedProcess(
            args[0],
            0,
            stdout=(
                "Type[3]:\n"
                " - type[0] (i64) -> i64\n"
                " - type[1] (i32, i64, i32) -> i32\n"
                " - type[2] () -> nil\n"
                "Import[3]:\n"
                " - func[10] sig=0 <molt_runtime.molt_function_set_builtin> <- molt_runtime.molt_function_set_builtin\n"
                " - func[11] sig=1 <molt_runtime.molt_string_from_bytes> <- molt_runtime.molt_string_from_bytes\n"
                " - func[12] sig=2 <molt_runtime.molt_resource_on_free> <- molt_runtime.molt_resource_on_free\n"
            ),
            stderr="",
        )

    monkeypatch.setattr("molt.cli.subprocess.run", fake_run)

    assert _wasm_import_function_result_kinds(
        wasm_path, module_name="molt_runtime"
    ) == {
        "molt_function_set_builtin": "i64",
        "molt_string_from_bytes": "i32",
        "molt_resource_on_free": "nil",
    }


def test_wasm_import_function_signatures_parses_objdump_output(
    monkeypatch, tmp_path
) -> None:
    import subprocess

    from molt.cli import _wasm_import_function_signatures

    wasm_path = tmp_path / "app.wasm"
    wasm_path.write_bytes(b"\x00asm\x01\x00\x00\x00")

    monkeypatch.setattr("molt.cli.shutil.which", lambda name: "/opt/homebrew/bin/wasm-objdump")

    def fake_run(*args, **kwargs):
        return subprocess.CompletedProcess(
            args[0],
            0,
            stdout=(
                "Type[3]:\n"
                " - type[0] (i64) -> i64\n"
                " - type[1] (i32, i64, i32) -> i32\n"
                " - type[2] () -> nil\n"
                "Import[3]:\n"
                " - func[10] sig=0 <molt_runtime.molt_function_set_builtin> <- molt_runtime.molt_function_set_builtin\n"
                " - func[11] sig=1 <molt_runtime.molt_string_from_bytes> <- molt_runtime.molt_string_from_bytes\n"
                " - func[12] sig=2 <molt_runtime.molt_resource_on_free> <- molt_runtime.molt_resource_on_free\n"
            ),
            stderr="",
        )

    monkeypatch.setattr("molt.cli.subprocess.run", fake_run)

    assert _wasm_import_function_signatures(
        wasm_path, module_name="molt_runtime"
    ) == {
        "molt_function_set_builtin": {"params": ["i64"], "result": "i64"},
        "molt_string_from_bytes": {"params": ["i32", "i64", "i32"], "result": "i32"},
        "molt_resource_on_free": {"params": [], "result": "nil"},
    }


def test_wasm_export_function_signatures_parses_objdump_output(
    monkeypatch, tmp_path
) -> None:
    import subprocess

    from molt.cli import _wasm_export_function_signatures

    wasm_path = tmp_path / "runtime.wasm"
    wasm_path.write_bytes(b"\x00asm\x01\x00\x00\x00")

    monkeypatch.setattr("molt.cli.shutil.which", lambda name: "/opt/homebrew/bin/wasm-objdump")

    def fake_run(*args, **kwargs):
        return subprocess.CompletedProcess(
            args[0],
            0,
            stdout=(
                "Type[2]:\n"
                " - type[0] (i64, i64, i64) -> i64\n"
                " - type[1] (i32) -> i32\n"
                "Function[2]:\n"
                " - func[17] sig=0 <__molt_table_ref_999>\n"
                " - func[18] sig=1 <__molt_table_ref_111>\n"
                "Export[2]:\n"
                ' - func[17] <__molt_table_ref_999> -> "__molt_table_ref_7"\n'
                ' - func[18] <__molt_table_ref_111> -> "__molt_table_ref_8"\n'
            ),
            stderr="",
        )

    monkeypatch.setattr("molt.cli.subprocess.run", fake_run)

    assert _wasm_export_function_signatures(
        wasm_path, export_name_prefix="__molt_table_ref_"
    ) == {
        "__molt_table_ref_7": {"params": ["i64", "i64", "i64"], "result": "i64"},
        "__molt_table_ref_8": {"params": ["i32"], "result": "i32"},
    }


def test_export_wasm_table_refs_adds_exports_for_active_slots(tmp_path) -> None:
    from molt.cli import (
        _build_wasm_sections,
        _export_wasm_table_refs,
        _parse_wasm_sections,
        _read_wasm_string,
        _read_wasm_varuint,
        _write_wasm_varuint,
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
    code_payload = _write_wasm_varuint(1) + _write_wasm_varuint(len(code_body)) + code_body
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

    exports = {}
    for section_id, payload in _parse_wasm_sections(wasm_path.read_bytes()):
        if section_id != 7:
            continue
        offset = 0
        count, offset = _read_wasm_varuint(payload, offset)
        for _ in range(count):
            name, offset = _read_wasm_string(payload, offset)
            kind = payload[offset]
            offset += 1
            index, offset = _read_wasm_varuint(payload, offset)
            exports[name] = (kind, index)
    assert exports["__molt_table_ref_3"] == (0, 0)
