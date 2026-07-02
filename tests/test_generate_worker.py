import json
import re
import shutil
import subprocess
from pathlib import Path

from molt._wasm_abi_generated import (
    WASM_RESERVED_RUNTIME_CALLABLES,
    WASM_TABLE_REF_EXPORT_PREFIX,
)
from molt.wasm_artifact import wasm_table_ref_export_name


def _table_ref_export_name(index: int) -> str:
    return wasm_table_ref_export_name(index)


def _reserved_runtime_callable_manifest_entries() -> list[dict[str, object]]:
    return [
        {
            "index": index,
            "runtime_export": runtime_name,
            "import_name": import_name,
            "arity": arity,
            "dispatch": dispatch,
        }
        for (
            index,
            runtime_name,
            import_name,
            arity,
            dispatch,
        ) in WASM_RESERVED_RUNTIME_CALLABLES
    ]


def _reserved_runtime_callable_js_entries(content: str) -> list[dict[str, object]]:
    match = re.search(
        r"const reservedRuntimeCallables = \[(?P<body>.*?)\];",
        content,
        re.DOTALL,
    )
    assert match is not None
    entries = []
    for item in re.finditer(
        (
            r"\{\s*index:\s*(?P<index>\d+),\s*"
            r"runtimeExport:\s*'(?P<runtime>[^']+)',\s*"
            r"arity:\s*(?P<arity>\d+)"
            r"(?:,\s*dispatch:\s*'(?P<dispatch>[^']+)')?\s*\}"
        ),
        match.group("body"),
    ):
        entries.append(
            {
                "index": int(item.group("index")),
                "runtime_export": item.group("runtime"),
                "arity": int(item.group("arity")),
                "dispatch": item.group("dispatch") or "direct",
            }
        )
    return entries


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
    assert "const writeBytesToFileEntry = (entry, bytes) => {" in content
    assert "writeBytesToFileEntry(entry, bytes);" in content
    assert "if (fdNum !== 1 && fdNum !== 2) {" in content


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


def test_generate_split_worker_installs_manifest_table_refs_before_main_wrapper() -> (
    None
):
    from molt.cli import _generate_split_worker_js

    app_ref = _table_ref_export_name(7)
    runtime_ref = _table_ref_export_name(3)
    content = _generate_split_worker_js(
        shared_memory_initial_pages=8,
        shared_table_initial=16,
        shared_table_base=32,
        app_table_ref_signatures={app_ref: {"params": ["i64"], "result": "i64"}},
        runtime_table_ref_signatures={
            runtime_ref: {"params": ["i32"], "result": "i32"}
        },
    )

    assert "const installTableRefs = (instance, table) => {" in content
    assert (
        "const ensureTableCapacityForExportedRefs = (instance, table) => {" in content
    )
    assert "installTableRefs(rtInstance, sharedTable);" in content
    assert "if (table.get(ref.index) !== null)" in content
    assert "ensureTableCapacityForExportedRefs(appInstance, sharedTable);" in content
    assert (
        "if (appInstance.exports.molt_table_init) appInstance.exports.molt_table_init();"
        in content
    )
    assert "installTableRefs(appInstance, sharedTable);" in content
    assert content.index(
        "if (appInstance.exports.molt_table_init) appInstance.exports.molt_table_init();"
    ) < content.index("installTableRefs(appInstance, sharedTable);")
    assert content.index("installTableRefs(appInstance, sharedTable);") < content.index(
        "if (appInstance.exports.molt_main) appInstance.exports.molt_main();"
    )
    assert (
        "App-owned table slots are initialized by the exported molt_main wrapper."
        not in content
    )
    assert "? [`MOLT_WASM_TABLE_BASE=${32}`]" in content
    assert (
        f"const TABLE_REF_EXPORT_PREFIX = {json.dumps(WASM_TABLE_REF_EXPORT_PREFIX)};"
        in content
    )
    assert "const parseTableRefExportName = (name) => {" in content
    assert "const tableRefExportName = (index) =>" in content


def test_generate_split_worker_uses_phased_call_indirect_routing() -> None:
    from molt.cli import _generate_split_worker_js
    from molt.cli.wasm import _split_runtime_browser_abi_from_manifest

    app_ref = _table_ref_export_name(7)
    runtime_ref = _table_ref_export_name(3)
    content = _generate_split_worker_js(
        shared_memory_initial_pages=8,
        shared_table_initial=16,
        shared_table_base=32,
        app_table_ref_signatures={app_ref: {"params": ["i64"], "result": "i64"}},
        runtime_table_ref_signatures={
            runtime_ref: {"params": ["i32"], "result": "i32"}
        },
    )

    assert 'const callIndirectImportNames = ["molt_call_indirect0"' in content
    assert "const reservedRuntimeCallables = [" in content
    assert "appOwnsReservedTrampoline" not in content
    assert '"runtime_export":"molt_object_init_subclass"' in content.replace(" ", "")
    assert "for (const indirectName of callIndirectImportNames) {" in content
    assert "hostEnv[indirectName] = (fnIndex, ...args) => {" in content
    assert "const idx = Number(fnIndex);" in content
    assert "const dispatchIdx = remapLegacyRuntimeSharedIdx(idx);" in content
    assert "const directName = tableRefExportName(dispatchIdx);" in content
    assert "const reservedDispatch = planReservedRuntimeDispatch({" in content
    assert (
        "const reservedRuntimeCallable = reservedDispatch.reservedRuntimeCallable;"
        in content
    )
    assert "return callReservedRuntimeCallable(" in content
    assert f"/^{WASM_TABLE_REF_EXPORT_PREFIX}" not in content
    assert (
        "const callIndirectObjectSignature = (name, { includeIndex = false } = {}) => {"
        in content
    )
    assert "const appDirectFn = appInstance?.exports?.[directName];" in content
    assert 'if (typeof appDirectFn === "function") {' in content
    assert (
        "appTableRefSignatures[directName] || callIndirectObjectSignature(indirectName)"
        in content
    )
    assert "app direct export ${directName} failed at idx=${idx}" in content
    assert "const indirectFn = appInstance?.exports?.[indirectName];" in content
    assert (
        "callIndirectObjectSignature(indirectName, { includeIndex: true })" in content
    )
    assert "const tableFn = sharedTable.get(dispatchIdx);" in content
    assert (
        "const directSignature = appTableRefSignatures[directName] || runtimeTableRefSignatures[directName] || null;"
        in content
    )
    assert 'if (typeof tableFn === "function" && directSignature) {' in content
    assert "return callWithSignature(tableFn, directSignature, args);" in content
    assert 'if (typeof tableFn === "function") {' in content
    assert (
        "return callWithSignature(tableFn, callIndirectObjectSignature(indirectName), args);"
        in content
    )
    assert "const rtDirectFn = rtInstance?.exports?.[directName];" in content
    assert (
        "const runtimeDirectSignature = runtimeTableRefSignatures[directName] || null;"
        in content
    )
    assert (
        'if (typeof rtDirectFn === "function" && runtimeDirectSignature) {' in content
    )
    assert (
        "return callWithSignature(rtDirectFn, runtimeDirectSignature, args);" in content
    )
    assert (
        "callIndirectObjectSignature(indirectName) || appTableRefSignatures[directName]"
        not in content
    )
    assert (
        "runtimeTableRefSignatures[directName] || callIndirectObjectSignature(indirectName)"
        not in content
    )
    assert content.index(
        "const reservedDispatch = planReservedRuntimeDispatch({"
    ) < content.index("const appDirectFn = appInstance?.exports?.[directName];")
    assert content.index(
        "const reservedDispatch = planReservedRuntimeDispatch({"
    ) < content.index("const indirectFn = appInstance?.exports?.[indirectName];")
    assert content.index(
        "const appDirectFn = appInstance?.exports?.[directName];"
    ) < content.index("const indirectFn = appInstance?.exports?.[indirectName];")
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
    assert "installTableRefs(appInstance, sharedTable);" in content
    browser_abi = _split_runtime_browser_abi_from_manifest()
    assert browser_abi["reserved_runtime_callables"] == (
        _reserved_runtime_callable_manifest_entries()
    )


def test_static_browser_runners_reserved_runtime_callable_tables_track_generated_abi() -> (
    None
):
    from pathlib import Path

    root = Path(__file__).resolve().parents[1]
    expected = [
        {
            "index": index,
            "runtime_export": runtime_name,
            "arity": arity,
            "dispatch": dispatch,
        }
        for (
            index,
            runtime_name,
            _import_name,
            arity,
            dispatch,
        ) in WASM_RESERVED_RUNTIME_CALLABLES
    ]
    for rel in ("wasm/run_wasm.js", "wasm/browser_host.js"):
        content = (root / rel).read_text(encoding="utf-8")
        assert _reserved_runtime_callable_js_entries(content) == expected


def test_loader_bridge_enforces_manifest_reserved_callable_dispatch(tmp_path) -> None:
    import pytest

    if shutil.which("node") is None:
        pytest.skip("node is required for loader bridge dispatch test")

    root = Path(__file__).resolve().parents[1]
    script = tmp_path / "check_reserved_dispatch.js"
    script.write_text(
        f"""
const bridge = require({str(root / "wasm" / "loader_bridge.js")!r});
const manifest = {{
  abi: {{
    browser_embed: {{
      reserved_runtime_callables: [{{
        index: 0,
        runtime_export: 'molt_importlib_import_transaction',
        import_name: 'molt_importlib_import_transaction',
        arity: 5,
        dispatch: 'trampoline',
      }}],
    }},
  }},
}};
const entries = bridge.reservedRuntimeCallablesFromManifest(manifest);
if (entries[0].dispatch !== 'trampoline') {{
  throw new Error(`unexpected dispatch ${{entries[0].dispatch}}`);
}}
const base = {{
  sharedTableBase: 100,
  reservedRuntimeCallableBase: 33,
  reservedRuntimeCallableCount: 1,
  reservedRuntimeCallables: entries,
}};
try {{
  bridge.planReservedRuntimeDispatch({{ ...base, dispatchIdx: 133 }});
  throw new Error('direct slot unexpectedly accepted');
}} catch (err) {{
  if (!String(err.message || err).includes('trampoline-only')) throw err;
}}
const plan = bridge.planReservedRuntimeDispatch({{ ...base, dispatchIdx: 134 }});
if (!plan.dispatchReservedRuntimeCallable || !plan.reservedRuntimeCallable.trampoline) {{
  throw new Error('trampoline slot was not routed through reserved callable');
}}
""".lstrip(),
        encoding="utf-8",
    )
    run = subprocess.run(
        ["node", str(script)],
        cwd=root,
        text=True,
        capture_output=True,
        timeout=30,
        check=False,
    )
    assert run.returncode == 0, run.stderr


def test_static_browser_host_fd_write_preserves_tmp_file_bytes() -> None:
    from pathlib import Path

    root = Path(__file__).resolve().parents[1]
    content = (root / "wasm/browser_host.js").read_text(encoding="utf-8")
    assert "const writeBytesToFileEntry = (entry, bytes) => {" in content
    assert "writeBytesToFileEntry(entry, bytes);" in content
    assert "if (fdNum !== 1 && fdNum !== 2) {" in content


def test_generate_split_worker_builds_runtime_import_wrappers_from_app_surface() -> (
    None
):
    from molt.cli import _generate_split_worker_js

    content = _generate_split_worker_js(
        shared_memory_initial_pages=8,
        shared_table_initial=16,
        shared_table_base=32,
        runtime_import_names={"function_set_builtin", "string_from_bytes"},
        runtime_export_signatures={
            "function_set_builtin": {"params": ["i64"], "result": "i64"},
            "string_from_bytes": {
                "params": ["i64", "i64", "i64"],
                "result": "i64",
            },
        },
        app_table_ref_signatures={
            _table_ref_export_name(1): {"params": ["i64"], "result": "i64"}
        },
        runtime_table_ref_signatures={
            _table_ref_export_name(2): {"params": ["i32"], "result": "i32"}
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
    assert (
        'const runtimeImportExportNames = {"function_set_builtin": "molt_function_set_builtin", "string_from_bytes": "molt_string_from_bytes"};'
        in content
    )
    assert (
        'const runtimeExportSignatures = {"function_set_builtin": {"params": ["i64"], "result": "i64"}, "string_from_bytes": {"params": ["i64", "i64", "i64"], "result": "i64"}};'
        in content
    )
    assert "const runtimeImportFallbacks =" in content
    assert '"fast_dict_get": {"call_arity": 2' in content
    assert '"strategy": "call_bind_ic"' in content
    assert '"dict_getitem": {"call_arity": null' in content
    assert '"exports": ["molt_dict_getitem_borrowed"]' in content
    expected_app_refs = {
        _table_ref_export_name(1): {"params": ["i64"], "result": "i64"}
    }
    expected_runtime_refs = {
        _table_ref_export_name(2): {"params": ["i32"], "result": "i32"}
    }
    assert (
        f"const appTableRefSignatures = {json.dumps(expected_app_refs, sort_keys=True)};"
        in content
    )
    assert (
        f"const runtimeTableRefSignatures = {json.dumps(expected_runtime_refs, sort_keys=True)};"
        in content
    )
    assert "const TAG_NONE = 0x0003000000000000n;" in content
    assert "const NONE_BITS = QNAN | TAG_NONE;" in content
    assert (
        "const exportSignature = runtimeExportSignatures[entry.name] || null;"
        in content
    )
    assert (
        "const signature = runtimeImportSignatures[entry.name] || exportSignature;"
        in content
    )
    assert (
        "const resultKind = runtimeImportResultKinds[entry.name] || (signature ? signature.result : null);"
        in content
    )
    assert "const exportName = runtimeImportExportNames[entry.name] || null;" in content
    assert "exportCandidates" not in content
    assert "`molt_${entry.name}`" not in content
    assert (
        "let callSignature = runtimeExportSignatures[entry.name] || signature;"
        in content
    )
    assert "fn = runtimeFallback(entry.name);" in content
    assert "callSignature = signature;" in content
    assert 'entry.name === "fast_dict_get"' not in content
    assert (
        "? args.map((value, index) => normalizeValueForKind(value, callSignature.params[index] || null))"
        in content
    )
    assert "const callArgs = args.map((value, index) =>" in content
    assert (
        "normalizeValueForKind(value, callSignature.params[index] || null)" in content
    )
    assert "return normalizeImportResult(out, resultKind);" in content
    assert "const callWithSignature = (fn, signature, args) => {" in content
    assert "value === undefined || value === null" in content
    assert "? NONE_BITS" in content
    assert "const callIsolateImportExport = (fn, args) => {" in content
    assert "molt_isolate_import expects one i64 handle" in content
    assert (
        "return callIsolateImportExport(appInstance.exports.molt_isolate_import, args);"
        in content
    )
    assert "molt_runtime: buildRuntimeImports(appModule, rtInstance)," in content
    assert "const runtimeAbiExports = (exports) => {" not in content


def test_static_js_isolate_import_bridges_use_single_i64_handle() -> None:
    from pathlib import Path

    root = Path(__file__).resolve().parents[1]
    bridge = (root / "wasm/loader_bridge.js").read_text(encoding="utf-8")
    assert "const callIsolateImportExport = (fn, args) => {" in bridge
    assert "normalizeI64BridgeValue(fn(handle), 'molt_isolate_import result')" in bridge
    shared_loader_surfaces = {
        "wasm/browser_embed.js": "globalThis.MoltWasmLoaderBridge",
        "wasm/browser_host.js": "globalThis.MoltWasmLoaderBridge",
        "wasm/run_wasm.js": "require('./loader_bridge.js')",
    }
    generated_surfaces = {
        "src/molt/cli/wasm.py": "normalizeI64BridgeValue",
    }
    forbidden = (
        ".exports.molt_isolate_import(...args)",
        ".exports.molt_isolate_import(...callArgs)",
    )
    for rel, loader_authority in shared_loader_surfaces.items():
        content = (root / rel).read_text(encoding="utf-8")
        assert loader_authority in content
        assert "callIsolateImportExport" in content
        for needle in forbidden:
            assert needle not in content
    for rel, helper in generated_surfaces.items():
        content = (root / rel).read_text(encoding="utf-8")
        assert "const callIsolateImportExport = (fn, args) => {" in content
        assert helper in content
        assert "molt_isolate_import expects one i64 handle" in content
        for needle in forbidden:
            assert needle not in content
    browser_embed = (root / "wasm/browser_embed.js").read_text(encoding="utf-8")
    assert (
        "const runtimeExportSignatures = runtimeImports.runtime_export_signatures || {};"
        in browser_embed
    )
    assert (
        "const runtimeExportNames = runtimeImports.export_names || {};" in browser_embed
    )
    assert (
        "const exportSignature = runtimeExportSignatures[entry.name] || null;"
        in browser_embed
    )
    assert (
        "const signature = signatures[entry.name] || exportSignature;" in browser_embed
    )
    assert (
        "const resultKind = resultKinds[entry.name] || signature.result || null;"
        in browser_embed
    )
    assert "const exportName = runtimeExportNames[entry.name] || null;" in browser_embed
    assert "exportCandidates" not in browser_embed
    assert "`molt_${entry.name}`" not in browser_embed
    assert (
        "let callSignature = runtimeExportSignatures[entry.name] || signature;"
        in browser_embed
    )
    assert (
        "normalizeValueForKind(value, callSignature.params[index] || null)"
        in browser_embed
    )
    run_wasm = (root / "wasm/run_wasm.js").read_text(encoding="utf-8")
    assert "const wasmAbiGenerated = require('./wasm_abi_generated.json');" in run_wasm
    assert (
        "const runtimeExportByImport = wasmAbiGenerated.runtime_export_by_import || {};"
        in run_wasm
    )
    assert (
        "const runtimeImportFallbacks = wasmAbiGenerated.runtime_import_fallbacks || {};"
        in run_wasm
    )
    assert "let runtimeImportExportNames = runtimeExportByImport;" in run_wasm
    assert (
        "const siblingRuntimeImportExportNames = siblingRuntimeImports.export_names || null;"
        in run_wasm
    )
    assert (
        "runtimeImportExportNames = siblingRuntimeImportExportNames\n"
        "    ? {\n"
        "        ...runtimeExportByImport,\n"
        "        ...siblingRuntimeImportExportNames,\n"
        "      }\n"
        "    : runtimeExportByImport;" in run_wasm
    )
    assert "generatedRuntimeExportByImport" not in run_wasm
    assert run_wasm.count("const runtimeExportNameForImport =") == 1
    assert "const runtimeExportNameForImport = (importName) => {" in run_wasm
    assert "const runtimeExport = runtimeExportNameForImport(entry.name);" in run_wasm
    assert "const runtimeExport = runtimeExportNameForImport(name);" in run_wasm
    assert "fn = runtimeFallbackFunction(runtimeInstance.exports, name);" in run_wasm
    assert "fn = runtimeFallbackFunction(runtimeInst.exports, entry.name);" in run_wasm
    assert "entry.name.startsWith('molt_')" not in run_wasm
    assert "`molt_${entry.name}`" not in run_wasm
    assert "`molt_${name}`" not in run_wasm


def test_static_wasm_loader_bridge_owns_binary_parser_authority() -> None:
    from pathlib import Path

    root = Path(__file__).resolve().parents[1]
    bridge = (root / "wasm/loader_bridge.js").read_text(encoding="utf-8")
    assert "const extractWasmTableBase = (buffer) => {" in bridge
    assert "const parseWasmExportFunctionSignatures = (buffer) => {" in bridge
    assert "const reservedRuntimeCallablesFromManifest = (manifest) => {" in bridge
    assert "extractWasmTableBase," in bridge
    assert "parseWasmExportFunctionSignatures," in bridge
    assert "reservedRuntimeCallablesFromManifest," in bridge

    consumers = {
        "wasm/browser_host.js": (
            "globalThis.MoltWasmLoaderBridge",
            "extractWasmTableBase,",
            "reservedRuntimeCallablesFromManifest,",
        ),
        "wasm/run_wasm.js": (
            "require('./loader_bridge.js')",
            "parseWasmExportFunctionSignatures: parseWasmExportFunctionSignaturesFromBridge",
            "reservedRuntimeCallablesFromManifest,",
        ),
    }
    forbidden_local_authority = (
        "const readVarUint =",
        "const readString =",
        "const readLimits =",
        "const readVarInt32 =",
        "const skipImportDesc =",
        "const extractWasmTableBase =",
        "const decodeWasmValType =",
        "const readWasmValTypeVec =",
        "const reservedRuntimeCallablesFromManifest =",
        "manifest?.abi?.browser_embed?.reserved_runtime_callables",
    )
    for rel, required in consumers.items():
        content = (root / rel).read_text(encoding="utf-8")
        for needle in required:
            assert needle in content
        for needle in forbidden_local_authority:
            assert needle not in content


def test_static_browser_hosts_publish_positive_pid_surrogate() -> None:
    from pathlib import Path

    root = Path(__file__).resolve().parents[1]
    for rel in ("wasm/browser_embed.js", "wasm/browser_host.js"):
        content = (root / rel).read_text(encoding="utf-8")
        assert "molt_getpid_host: () => 1n" in content
        assert "molt_getpid_host: stubZeroI64" not in content


def test_static_browser_host_split_runtime_imports_are_manifest_backed() -> None:
    from pathlib import Path

    root = Path(__file__).resolve().parents[1]
    browser_host = (root / "wasm/browser_host.js").read_text(encoding="utf-8")
    assert "normalizeImportResult," in browser_host
    assert "normalizeValueForKind," in browser_host
    assert (
        "const loadSplitRuntimeManifest = async (options, wasmUrl) => {" in browser_host
    )
    assert "split-runtime manifest missing abi.runtime_imports.names" in browser_host
    assert "const runtimeImportAbi = options.runtimeImportAbi || {};" in browser_host
    assert (
        "const manifestNames = new Set(runtimeImportAbi.names || []);" in browser_host
    )
    assert (
        "const runtimeExportSignatures = runtimeImportAbi.runtime_export_signatures || {};"
        in browser_host
    )
    assert (
        "const runtimeExportNames = runtimeImportAbi.export_names || {};"
        in browser_host
    )
    assert "const resultKinds = runtimeImportAbi.result_kinds || {};" in browser_host
    assert (
        "const runtimeImportFallbacks = options.runtimeImportFallbacks || {};"
        in browser_host
    )
    assert "app runtime import ${entry.name} missing from manifest" in browser_host
    assert "app runtime import ${entry.name} missing manifest signature" in browser_host
    assert (
        "app runtime import ${entry.name} missing manifest result kind" in browser_host
    )
    assert (
        "const exportSignature = runtimeExportSignatures[entry.name] || null;"
        in browser_host
    )
    assert (
        "const signature = signatures[entry.name] || exportSignature;" in browser_host
    )
    assert (
        "const resultKind = resultKinds[entry.name] || signature.result || null;"
        in browser_host
    )
    assert "const exportName = runtimeExportNames[entry.name] || null;" in browser_host
    assert "exportCandidates" not in browser_host
    assert "`molt_${entry.name}`" not in browser_host
    assert (
        "let callSignature = runtimeExportSignatures[entry.name] || signature;"
        in browser_host
    )
    assert "fn = resolveFallback(entry.name);" in browser_host
    assert (
        "normalizeValueForKind(value, callSignature.params[index] || null)"
        in browser_host
    )
    assert "return normalizeImportResult(fn(...callArgs), resultKind);" in browser_host
    assert "runtimeImportAbi," in browser_host
    assert "runtimeImportFallbacks," in browser_host
    assert "entry.name === 'fast_list_append'" not in browser_host
    assert "entry.name === 'dict_setitem'" not in browser_host


def test_effective_split_worker_table_base_uses_backend_authority() -> None:
    from molt.cli import _effective_split_worker_table_base

    assert (
        _effective_split_worker_table_base(
            wasm_table_base=4096,
            app_table_ref_signatures={
                _table_ref_export_name(4096): {
                    "params": ["i64"],
                    "result": "i64",
                },
                _table_ref_export_name(4189): {"params": ["i64"], "result": "i64"},
            },
        )
        == 4096
    )


def test_effective_split_worker_table_base_does_not_infer_fallback() -> None:
    from molt.cli import _effective_split_worker_table_base

    assert (
        _effective_split_worker_table_base(
            wasm_table_base=None,
            app_table_ref_signatures={
                _table_ref_export_name(4130): {"params": ["i64"], "result": "i64"},
            },
        )
        is None
    )


def test_effective_split_worker_table_base_rejects_export_below_backend_base() -> None:
    import pytest

    from molt.cli import _effective_split_worker_table_base

    with pytest.raises(ValueError, match="above exported table-ref slot"):
        _effective_split_worker_table_base(
            wasm_table_base=4096,
            app_table_ref_signatures={
                _table_ref_export_name(4095): {"params": ["i64"], "result": "i64"},
            },
        )


def test_effective_split_worker_table_base_rejects_active_slot_below_backend_base(
    tmp_path: Path,
) -> None:
    import pytest

    from molt.cli import _effective_split_worker_table_base

    app_wasm = tmp_path / "app.wasm"
    app_wasm.write_bytes(_active_table_fixture_wasm(base_slot=1024))

    with pytest.raises(ValueError, match="above active app table slot"):
        _effective_split_worker_table_base(
            wasm_table_base=4096,
            app_table_ref_signatures={},
            app_wasm=app_wasm,
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


def _active_table_fixture_wasm(*, base_slot: int) -> bytes:
    import molt.wasm_artifact as wasm_artifact

    type_payload = _wasm_vec([_wasm_function_type([], [])])
    function_payload = _wasm_vec([wasm_artifact._write_wasm_varuint(0)])
    table_payload = _wasm_vec(
        [b"\x70\x00" + wasm_artifact._write_wasm_varuint(max(base_slot + 1, 1))]
    )
    element_payload = _wasm_vec(
        [
            b"\x00\x41"
            + wasm_artifact._write_wasm_varuint(base_slot)
            + b"\x0b\x00"
            + wasm_artifact._write_wasm_varuint(1)
            + wasm_artifact._write_wasm_varuint(0)
        ]
    )
    code_body = b"\x00\x0b"
    code_payload = _wasm_vec(
        [wasm_artifact._write_wasm_varuint(len(code_body)) + code_body]
    )
    return wasm_artifact._build_wasm_sections(
        [
            (1, type_payload),
            (3, function_payload),
            (4, table_payload),
            (9, element_payload),
            (10, code_payload),
        ]
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
            _wasm_function_export(_table_ref_export_name(7), 3),
            _wasm_function_export(_table_ref_export_name(8), 4),
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


def test_runtime_import_signatures_are_manifest_backed() -> None:
    from molt.cli.wasm import (
        _runtime_import_result_kinds_from_manifest,
        _runtime_import_signatures_from_manifest,
    )

    import_names = {
        "abc_bootstrap",
        "socket_drop",
        "function_set_builtin",
        "molt_abc_bootstrap",
        "molt_importlib_import_transaction",
        "molt_socket_drop",
        "molt_add",
        "molt_bool_from_i32",
        "molt_buffer_acquire",
        "molt_string_from_bytes",
        "string_from_bytes",
    }

    assert _runtime_import_result_kinds_from_manifest(import_names) == {
        "abc_bootstrap": "i64",
        "socket_drop": "nil",
        "function_set_builtin": "i64",
        "molt_abc_bootstrap": "i64",
        "molt_importlib_import_transaction": "i64",
        "molt_socket_drop": "nil",
        "molt_add": "i64",
        "molt_bool_from_i32": "i64",
        "molt_buffer_acquire": "i32",
        "molt_string_from_bytes": "i32",
        "string_from_bytes": "i32",
    }
    assert _runtime_import_signatures_from_manifest(import_names) == {
        "abc_bootstrap": {"params": [], "result": "i64"},
        "socket_drop": {"params": ["i64"], "result": "nil"},
        "function_set_builtin": {"params": ["i64"], "result": "i64"},
        "molt_abc_bootstrap": {"params": [], "result": "i64"},
        "molt_importlib_import_transaction": {
            "params": ["i64", "i64", "i64", "i64", "i64"],
            "result": "i64",
        },
        "molt_socket_drop": {"params": ["i64"], "result": "nil"},
        "molt_add": {"params": ["i64", "i64"], "result": "i64"},
        "molt_bool_from_i32": {"params": ["i32"], "result": "i64"},
        "molt_buffer_acquire": {"params": ["i64", "i32"], "result": "i32"},
        "molt_string_from_bytes": {
            "params": ["i32", "i64", "i32"],
            "result": "i32",
        },
        "string_from_bytes": {"params": ["i32", "i64", "i32"], "result": "i32"},
    }


def test_cpython_abi_runtime_imports_use_runtime_export_signatures() -> None:
    import pytest

    from molt.cli.wasm import (
        _generate_split_worker_js,
        _runtime_import_export_names_from_manifest,
        _runtime_import_result_kinds_from_manifest,
        _runtime_import_signatures_from_manifest,
    )

    runtime_export_signatures = {
        "PyArg_ParseTuple": {"params": ["i32", "i32", "i32"], "result": "i32"},
        "PyFloat_Check": {"params": ["i32"], "result": "i32"},
    }
    import_names = {"PyArg_ParseTuple", "PyFloat_Check"}

    assert _runtime_import_result_kinds_from_manifest(
        import_names,
        runtime_export_signatures=runtime_export_signatures,
    ) == {
        "PyArg_ParseTuple": "i32",
        "PyFloat_Check": "i32",
    }
    assert (
        _runtime_import_signatures_from_manifest(
            import_names,
            runtime_export_signatures=runtime_export_signatures,
        )
        == runtime_export_signatures
    )

    with pytest.raises(ValueError, match="missing from WASM ABI manifest"):
        _runtime_import_signatures_from_manifest(
            {"NotARuntimeImport"},
            runtime_export_signatures={
                "NotARuntimeImport": {"params": [], "result": "i32"}
            },
        )

    assert _runtime_import_export_names_from_manifest(import_names) == {
        "PyArg_ParseTuple": "PyArg_ParseTuple",
        "PyFloat_Check": "PyFloat_Check",
    }

    worker_js = _generate_split_worker_js(
        shared_memory_initial_pages=1,
        shared_table_initial=8192,
        shared_table_base=None,
        runtime_import_names=import_names,
        runtime_export_signatures=runtime_export_signatures,
    )
    assert (
        '"PyArg_ParseTuple": {"params": ["i32", "i32", "i32"], "result": "i32"}'
        in worker_js
    )
    assert '"PyArg_ParseTuple": "PyArg_ParseTuple"' in worker_js
    assert "exportCandidates" not in worker_js
    assert "`molt_${entry.name}`" not in worker_js


def test_runtime_export_signatures_use_cpython_abi_raw_export_names(
    monkeypatch,
) -> None:
    import molt.cli.non_native_output as non_native_output

    requested: dict[str, set[str]] = {}

    def fake_export_signatures(runtime_wasm, *, export_names):
        requested["export_names"] = set(export_names)
        return {
            "PyArg_ParseTuple": {"params": ["i32", "i32", "i32"], "result": "i32"},
            "molt_socket_drop": {"params": ["i64"], "result": "nil"},
        }

    monkeypatch.setattr(
        non_native_output,
        "_wasm_export_function_signatures",
        fake_export_signatures,
    )

    assert non_native_output._runtime_export_signatures_for_imports(
        Path("runtime.wasm"),
        {"PyArg_ParseTuple", "socket_drop"},
    ) == {
        "PyArg_ParseTuple": {"params": ["i32", "i32", "i32"], "result": "i32"},
        "socket_drop": {"params": ["i64"], "result": "nil"},
    }
    assert requested["export_names"] == {"PyArg_ParseTuple", "molt_socket_drop"}


def test_wasm_export_function_signatures_reads_wasm_bytes(tmp_path) -> None:
    from molt.wasm_artifact import (
        _wasm_export_function_signatures,
        wasm_table_ref_export_signatures,
    )

    wasm_path = tmp_path / "runtime.wasm"
    wasm_path.write_bytes(_signature_fixture_wasm())

    assert wasm_table_ref_export_signatures(wasm_path) == {
        _table_ref_export_name(7): {"params": ["i64"], "result": "i64"},
        _table_ref_export_name(8): {"params": ["i32", "i64", "i32"], "result": "i32"},
    }
    assert _wasm_export_function_signatures(
        wasm_path, export_names={_table_ref_export_name(8)}
    ) == {
        _table_ref_export_name(8): {"params": ["i32", "i64", "i32"], "result": "i32"},
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
    assert exports[_table_ref_export_name(3)] == (0, 0)
