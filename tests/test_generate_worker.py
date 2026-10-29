from tests.wasm_execution_manifest import write_wasm_execution_manifest

from tests.wasm_execution_manifest import (
    EMPTY_WASM_MODULE,
    split_wasm_execution_manifest,
)

from tests.process_guard_common import run_guarded_test_process
import json
import re
import shutil
from pathlib import Path

import pytest
from molt._wasm_abi_generated import (
    WASM_DEFAULT_APP_TABLE_BASE,
    WASM_RESERVED_RUNTIME_CALLABLE_BASE,
    WASM_RESERVED_RUNTIME_CALLABLES,
    WASM_RESERVED_RUNTIME_CALLABLE_TRAMPOLINE_ABI_BY_RUNTIME,
)
from molt.browser_asset_closure import (
    BROWSER_WASM_ENTRY_ASSETS,
    wasm_loader_asset_closure,
)


@pytest.mark.parametrize(
    "version", ["missing", None, 0, 1, 2, 3, 99, True, False, "4", 4.0]
)
def test_worker_generators_reject_incompatible_receiver_before_module_read(
    tmp_path: Path, version: object
) -> None:
    from molt.cli.wasm import _generate_split_worker_js
    from tools.generate_worker import generate_worker

    manifest: dict[str, object] = {"mode": "linked", "modules": {}}
    if version != "missing":
        manifest["version"] = version
    path = tmp_path / "manifest.json"
    path.write_text(json.dumps(manifest), encoding="utf-8")
    output = tmp_path / "worker.js"
    with pytest.raises(ValueError, match="Molt deployment manifest version must be 4"):
        generate_worker(output, manifest=path)
    assert not output.exists()
    manifest["mode"] = "split-runtime"
    with pytest.raises(ValueError, match="Molt deployment manifest version must be 4"):
        _generate_split_worker_js(
            deployment_manifest=manifest,
            shared_memory_initial_pages=1,
            shared_table_initial=1,
            shared_table_base=None,
        )


def _reserved_runtime_callable_manifest_entries() -> list[dict[str, object]]:
    return [
        {
            "index": index,
            "runtime_export": runtime_name,
            "import_name": import_name,
            "arity": arity,
            "dispatch": dispatch,
            "trampoline_abi": WASM_RESERVED_RUNTIME_CALLABLE_TRAMPOLINE_ABI_BY_RUNTIME[
                runtime_name
            ],
        }
        for (
            index,
            runtime_name,
            import_name,
            arity,
            dispatch,
        ) in WASM_RESERVED_RUNTIME_CALLABLES
    ]


def test_generate_worker_produces_valid_js(tmp_path):
    from tools.generate_worker import generate_worker

    output = tmp_path / "worker.js"
    module = tmp_path / "worker_linked.wasm"
    module.write_bytes(EMPTY_WASM_MODULE)
    write_wasm_execution_manifest(tmp_path, linked=module)
    generate_worker(output, tmp_quota_mb=32)
    content = output.read_text(encoding="utf-8")
    assert "fetch" in content
    assert "WebAssembly" in content


def test_generate_worker_contains_tmpfs(tmp_path):
    from tools.generate_worker import generate_worker

    output = tmp_path / "worker.js"
    module = tmp_path / "worker_linked.wasm"
    module.write_bytes(EMPTY_WASM_MODULE)
    write_wasm_execution_manifest(tmp_path, linked=module)
    generate_worker(output, tmp_quota_mb=64)
    content = output.read_text(encoding="utf-8")
    assert "class TmpFs" in content
    assert "TMP_QUOTA_MB = 64" in content
    assert "ENOSPC: quota exceeded" in content


@pytest.mark.parametrize("debug_stderr", [False, True])
def test_generate_worker_debug_stderr_is_explicit_deployment_option(
    tmp_path, debug_stderr
):
    from tools.generate_worker import generate_worker

    module = tmp_path / "worker_linked.wasm"
    module.write_bytes(EMPTY_WASM_MODULE)
    write_wasm_execution_manifest(tmp_path, linked=module)
    output = tmp_path / "worker.js"
    generate_worker(output, debug_stderr=debug_stderr)
    source = output.read_text(encoding="utf-8")
    assert f"const DEBUG_STDERR = {str(debug_stderr).lower()};" in source
    assert "CAPABILITIES.includes('debug')" not in source


def test_generate_worker_cli_uses_debug_option_and_rejects_retired_capabilities(
    tmp_path, monkeypatch
):
    import sys
    from tools import generate_worker

    module = tmp_path / "worker_linked.wasm"
    module.write_bytes(EMPTY_WASM_MODULE)
    write_wasm_execution_manifest(tmp_path, linked=module)
    output = tmp_path / "worker.js"
    monkeypatch.setattr(
        sys, "argv", ["generate_worker.py", "-o", str(output), "--debug-stderr"]
    )
    assert generate_worker.main() == 0
    assert "const DEBUG_STDERR = true;" in output.read_text(encoding="utf-8")
    monkeypatch.setattr(
        sys, "argv", ["generate_worker.py", "--capabilities", "fs.read"]
    )
    with pytest.raises(SystemExit) as error:
        generate_worker.main()
    assert error.value.code == 2


def test_generate_worker_contains_host_imports(tmp_path):
    from tools.generate_worker import generate_worker

    output = tmp_path / "worker.js"
    module = tmp_path / "worker_linked.wasm"
    module.write_bytes(EMPTY_WASM_MODULE)
    write_wasm_execution_manifest(tmp_path, linked=module)
    generate_worker(output, tmp_quota_mb=16)
    content = output.read_text(encoding="utf-8")
    assert "createHostImports" in content
    assert "molt_vfs_read" in content
    assert "molt_vfs_write" in content
    assert "molt_log_host" in content


def test_generate_worker_contains_fetch_handler(tmp_path):
    from tools.generate_worker import generate_worker

    output = tmp_path / "worker.js"
    module = tmp_path / "worker_linked.wasm"
    module.write_bytes(EMPTY_WASM_MODULE)
    write_wasm_execution_manifest(tmp_path, linked=module)
    generate_worker(output)
    content = output.read_text(encoding="utf-8")
    assert "async fetch(request, env, ctx)" in content
    assert "export default" in content
    assert "molt_main" in content


def test_generate_worker_contains_wasi_shim(tmp_path):
    from tools.generate_worker import generate_worker

    output = tmp_path / "worker.js"
    module = tmp_path / "worker_linked.wasm"
    module.write_bytes(EMPTY_WASM_MODULE)
    write_wasm_execution_manifest(tmp_path, linked=module)
    generate_worker(output)
    content = output.read_text(encoding="utf-8")
    assert "buildWasiShim" in content
    assert "wasi_snapshot_preview1" in content
    assert "fd_write" in content
    assert "fd_read" in content
    assert "proc_exit" in content
    assert "clock_time_get" in content


def test_generate_worker_preserves_policy_and_substitutes_template_tokens(tmp_path):
    from tools.generate_worker import generate_worker
    from molt.capability_manifest import CapabilityManifest

    output = tmp_path / "worker.js"
    module = tmp_path / "worker_linked.wasm"
    module.write_bytes(EMPTY_WASM_MODULE)
    manifest = write_wasm_execution_manifest(
        tmp_path, linked=module, policy=CapabilityManifest(allow=["fs.read"]).resolve()
    )
    generate_worker(output, tmp_quota_mb=32)
    content = output.read_text(encoding="utf-8")
    deployment = json.loads(manifest.read_text(encoding="utf-8"))
    assert json.dumps(json.dumps(deployment, sort_keys=True)) in content
    assert "{{DEBUG_STDERR}}" not in content
    assert "{{TMP_QUOTA_MB}}" not in content
    assert "{{WASM_FILENAME}}" not in content
    assert "{{SYNTHETIC_ERRNO}}" not in content
    assert "{{WASI_PREVIEW1_ERRNO}}" not in content


def test_generate_worker_custom_wasm_filename(tmp_path):
    from tools.generate_worker import generate_worker

    output = tmp_path / "worker.js"
    module = tmp_path / "custom.wasm"
    module.write_bytes(EMPTY_WASM_MODULE)
    write_wasm_execution_manifest(tmp_path, linked=module)
    generate_worker(output, wasm_filename="custom.wasm")
    content = output.read_text(encoding="utf-8")
    assert "custom.wasm" in content
    assert "worker_linked.wasm" not in content


def test_generate_worker_no_scaffold_warning(tmp_path):
    from tools.generate_worker import generate_worker

    output = tmp_path / "worker.js"
    module = tmp_path / "worker_linked.wasm"
    module.write_bytes(EMPTY_WASM_MODULE)
    write_wasm_execution_manifest(tmp_path, linked=module)
    generate_worker(output)
    content = output.read_text(encoding="utf-8")
    assert "SCAFFOLD" not in content
    assert "NOT PRODUCTION READY" not in content


def test_generate_worker_stdio_capture(tmp_path):
    from tools.generate_worker import generate_worker

    output = tmp_path / "worker.js"
    module = tmp_path / "worker_linked.wasm"
    module.write_bytes(EMPTY_WASM_MODULE)
    write_wasm_execution_manifest(tmp_path, linked=module)
    generate_worker(output)
    content = output.read_text(encoding="utf-8")
    assert "class StdioCapture" in content
    assert "writeStdout" in content
    assert "writeStderr" in content
    assert "readStdin" in content


def test_generate_split_worker_bootstrap_import_surface() -> None:
    from molt.cli.wasm import _generate_split_worker_js

    content = _generate_split_worker_js(
        deployment_manifest=split_wasm_execution_manifest(
            EMPTY_WASM_MODULE, EMPTY_WASM_MODULE
        ),
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
    from molt.cli.wasm import _generate_split_worker_js

    content = _generate_split_worker_js(
        deployment_manifest=split_wasm_execution_manifest(
            EMPTY_WASM_MODULE, EMPTY_WASM_MODULE
        ),
        shared_memory_initial_pages=8,
        shared_table_initial=16,
        shared_table_base=None,
    )

    assert 'import "./molt_vfs_browser.js";' in content
    assert "new globalThis.MoltVfs()" in content


def test_generate_split_worker_contains_vfs_host_imports() -> None:
    from molt.cli.wasm import _generate_split_worker_js

    content = _generate_split_worker_js(
        deployment_manifest=split_wasm_execution_manifest(
            EMPTY_WASM_MODULE, EMPTY_WASM_MODULE
        ),
        shared_memory_initial_pages=8,
        shared_table_initial=16,
        shared_table_base=32,
    )

    assert "molt_vfs_read" in content
    assert "molt_vfs_write" in content
    assert "molt_vfs_exists" in content
    assert "molt_vfs_unlink" in content


def test_generate_split_worker_uses_worker_env_for_static_assets() -> None:
    from molt.cli.wasm import _generate_split_worker_js

    content = _generate_split_worker_js(
        deployment_manifest=split_wasm_execution_manifest(
            EMPTY_WASM_MODULE, EMPTY_WASM_MODULE
        ),
        shared_memory_initial_pages=8,
        shared_table_initial=16,
        shared_table_base=None,
    )

    assert "async fetch(request, env, ctx)" in content
    assert "env.__STATIC_CONTENT" in content


def test_generate_split_worker_defines_utf8_decoder_for_vfs_paths() -> None:
    from molt.cli.wasm import _generate_split_worker_js

    content = _generate_split_worker_js(
        deployment_manifest=split_wasm_execution_manifest(
            EMPTY_WASM_MODULE, EMPTY_WASM_MODULE
        ),
        shared_memory_initial_pages=8,
        shared_table_initial=16,
        shared_table_base=None,
    )

    assert "const utf8Decoder = new TextDecoder();" in content
    assert "utf8Decoder.decode" in content
    assert "UTF8_DECODER" not in content


def test_generate_split_worker_defines_wasi_vfs_errno_and_preopen_state() -> None:
    from molt.cli.wasm import _generate_split_worker_js

    content = _generate_split_worker_js(
        deployment_manifest=split_wasm_execution_manifest(
            EMPTY_WASM_MODULE, EMPTY_WASM_MODULE
        ),
        shared_memory_initial_pages=8,
        shared_table_initial=16,
        shared_table_base=None,
    )

    assert "ENOENT: WASI_ERRNO_NOENT" in content
    assert "ENOSYS: WASI_ERRNO_NOSYS" in content
    assert "} = WASI_ERRNO;" in content
    assert "const WASI_OFLAGS_CREAT = 1;" in content
    assert "const wasiFiles = new Map();" in content
    assert "const wasiPreopens = [" in content
    assert "const preopenByFd = (fdNum) =>" in content
    assert "clock_res_get(id, outPtr)" in content
    assert "proc_raise() { return WASI_ERRNO_NOSYS; }" in content
    assert "fd_filestat_set_times: wasiUnsupported" in content


def test_generate_split_worker_replaces_path_stubs_with_vfs_backed_wasi_ops() -> None:
    from molt.cli.wasm import _generate_split_worker_js

    content = _generate_split_worker_js(
        deployment_manifest=split_wasm_execution_manifest(
            EMPTY_WASM_MODULE, EMPTY_WASM_MODULE
        ),
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
    from molt.cli.wasm import _generate_split_wrangler_jsonc

    root = Path(__file__).resolve().parents[1]
    browser_assets = wasm_loader_asset_closure(root / "wasm", BROWSER_WASM_ENTRY_ASSETS)
    content = _generate_split_wrangler_jsonc("2026-04-11", browser_assets)
    payload = json.loads(content)

    assert '"main": "worker.js"' in content
    assert '"find_additional_modules": true' in content
    assert payload["rules"][0]["globs"] == ["worker.js", *sorted(browser_assets)]
    assert '"globs": ["app.wasm", "molt_runtime.wasm"]' in content
    assert '"**/*.js"' not in content
    assert '"**/*.wasm"' not in content
    assert "output.wasm" not in content
    assert "output_linked.wasm" not in content


def test_generate_split_worker_requires_final_callable_table_before_main() -> None:
    from molt.cli.wasm import _generate_split_worker_js

    content = _generate_split_worker_js(
        deployment_manifest=split_wasm_execution_manifest(
            EMPTY_WASM_MODULE, EMPTY_WASM_MODULE
        ),
        shared_memory_initial_pages=8,
        shared_table_initial=16,
        shared_table_base=32,
    )

    assert 'import "./callable_table_abi_generated.js";' in content
    assert 'import "./loader_bridge.js";' in content
    assert 'callableTableFromModule(runtimeModule, "runtime wasm")' in content
    assert 'callableTableFromModule(appModule, "app wasm")' in content
    assert (
        'verifyCallableTableEntries(runtimeCallableTable, sharedTable, "runtime wasm");'
        in content
    )
    assert (
        'verifyCallableTableEntries(appCallableTable, sharedTable, "app wasm");'
        in content
    )
    assert "molt_table_init" not in content
    assert "installTableRefs" not in content
    assert "__molt_table_ref_" not in content


def test_generate_split_worker_uses_phased_call_indirect_routing() -> None:
    from molt.cli.wasm import _generate_split_worker_js
    from molt.cli.wasm import _split_runtime_browser_abi_from_manifest

    content = _generate_split_worker_js(
        deployment_manifest=split_wasm_execution_manifest(
            EMPTY_WASM_MODULE, EMPTY_WASM_MODULE
        ),
        shared_memory_initial_pages=8,
        shared_table_initial=16,
        shared_table_base=32,
    )

    assert 'const callIndirectImportNames = ["molt_call_indirect0"' in content
    assert "const reservedRuntimeCallables = [" in content
    assert "appOwnsReservedTrampoline" not in content
    assert '"runtime_export":"molt_object_init_subclass"' in content.replace(" ", "")
    assert "for (const indirectName of callIndirectImportNames) {" in content
    assert "hostEnv[indirectName] = (fnIndex, ...args) => {" in content
    assert "const idx = Number(fnIndex);" in content
    assert "const dispatchIdx = remapDefaultAppRuntimeSharedIdx(idx);" in content
    assert "const reservedDispatch = planReservedRuntimeDispatch({" in content
    assert (
        "const reservedRuntimeCallable = reservedDispatch.reservedRuntimeCallable;"
        in content
    )
    assert "return callReservedRuntimeCallable(" in content
    assert "const tableFn = sharedTable.get(dispatchIdx);" in content
    assert (
        "const directSignature =\n"
        "          callableTableSignature(appCallableTable, dispatchIdx) ||\n"
        "          callableTableSignature(runtimeCallableTable, dispatchIdx);"
        in content
    )
    assert 'if (typeof tableFn === "function" && directSignature) {' in content
    assert "return callWithSignature(tableFn, directSignature, args);" in content
    assert "has no callable signature authority" in content
    assert "appDirectFn" not in content
    assert "rtDirectFn" not in content
    assert "appTableRefSignatures" not in content
    assert "runtimeTableRefSignatures" not in content
    assert "__molt_table_ref_" not in content
    assert "molt_table_init" not in content
    assert (
        'verifyCallableTableEntries(appCallableTable, sharedTable, "app wasm");'
        in content
    )
    browser_abi = _split_runtime_browser_abi_from_manifest()
    assert browser_abi["reserved_runtime_callables"] == (
        _reserved_runtime_callable_manifest_entries()
    )


def test_static_browser_runners_use_the_generated_reserved_callable_layout() -> None:
    """Every JS host dispatches reserved runtime callables by the generated layout.

    run_wasm.js and browser_host.js used to carry their own base (33) and a
    24-entry list while the ABI moved to base 30 and 37 entries, so split-runtime
    calls landed on the wrong runtime callable (``new_class() expects positional
    arguments tuple`` while constructing a ModuleSpec).
    """
    from tests.wasm_linked_runner import require_node_binary

    root = Path(__file__).resolve().parents[1]
    result = run_guarded_test_process(
        [
            require_node_binary(),
            "-e",
            "const b = require('./wasm/loader_bridge.js');"
            "const l = b.runtimeCallableTableLayout(null);"
            "process.stdout.write(JSON.stringify(l));",
        ],
        prefix="MOLT_WASM_TEST",
        cwd=root,
        capture_output=True,
        text=True,
        timeout=30,
    )
    assert result.returncode == 0, result.stderr
    layout = json.loads(result.stdout)
    assert layout["defaultAppTableBase"] == WASM_DEFAULT_APP_TABLE_BASE
    assert layout["reservedRuntimeCallableBase"] == WASM_RESERVED_RUNTIME_CALLABLE_BASE
    assert layout["reservedRuntimeCallables"] == [
        {
            "index": index,
            "runtimeExport": runtime_name,
            "importName": import_name,
            "arity": arity,
            "dispatch": dispatch,
            "trampolineAbi": WASM_RESERVED_RUNTIME_CALLABLE_TRAMPOLINE_ABI_BY_RUNTIME[
                runtime_name
            ],
        }
        for index, runtime_name, import_name, arity, dispatch in (
            WASM_RESERVED_RUNTIME_CALLABLES
        )
    ]


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
    run = run_guarded_test_process(
        ["node", str(script)],
        cwd=root,
        text=True,
        capture_output=True,
        timeout=30,
        check=False,
    )
    assert run.returncode == 0, run.stderr


def test_loader_bridge_installs_manifest_link_import_traps(tmp_path) -> None:
    import pytest

    if shutil.which("node") is None:
        pytest.skip("node is required for loader bridge link-import test")

    root = Path(__file__).resolve().parents[1]
    script = tmp_path / "check_link_import_traps.js"
    script.write_text(
        f"""
const bridge = require({str(root / "wasm" / "loader_bridge.js")!r});
const importObject = {{ env: {{ memory: new WebAssembly.Memory({{ initial: 1 }}) }} }};
const imports = {{
  funcImports: [
    {{ module: 'env', name: '_ZNSt3__217bad_function_callD1Ev' }},
    {{ module: 'env', name: '_ZTVNSt3__217bad_function_callE' }},
    {{ module: 'env', name: 'unknown_not_manifested' }},
  ],
}};
bridge.installManifestLinkImportTraps(importObject, imports, {{
  primitive_classes: {{
    _ZNSt3__217bad_function_callD1Ev: 'wasm_libcxx_link_import',
    _ZTVNSt3__217bad_function_callE: 'wasm_libcxx_link_import',
  }},
  symbol_kinds: {{
    _ZTVNSt3__217bad_function_callE: 'data',
  }},
}});
if (typeof importObject.env._ZNSt3__217bad_function_callD1Ev !== 'function') {{
  throw new Error('function link import was not installed');
}}
if (Object.prototype.hasOwnProperty.call(importObject.env, '_ZTVNSt3__217bad_function_callE')) {{
  throw new Error('data link import was installed as a function');
}}
if (Object.prototype.hasOwnProperty.call(importObject.env, 'unknown_not_manifested')) {{
  throw new Error('unknown import was installed');
}}
try {{
  importObject.env._ZNSt3__217bad_function_callD1Ev();
  throw new Error('trap did not throw');
}} catch (err) {{
  if (!String(err.message || err).includes('wasm_libcxx_link_import')) throw err;
}}
""".lstrip(),
        encoding="utf-8",
    )
    run = run_guarded_test_process(
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
    from molt.cli.wasm import _generate_split_worker_js

    content = _generate_split_worker_js(
        deployment_manifest=split_wasm_execution_manifest(
            EMPTY_WASM_MODULE, EMPTY_WASM_MODULE
        ),
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
    )

    assert (
        "const buildRuntimeImports = (module, runtimeInstance) => bindRuntimeFunctionImports("
        in content
    )
    assert "WebAssembly.Module.imports(module), runtimeInstance.exports," in content
    assert "const boxInt =" not in content
    assert "const TAG_INT =" not in content
    assert "const INT_MASK =" not in content

    def declaration(name: str):
        match = re.search(rf"const {name} = (.*);", content)
        assert match is not None
        return json.loads(match[1])

    result_kinds = declaration("runtimeImportResultKinds")
    signatures = declaration("runtimeImportSignatures")
    exports = declaration("runtimeImportExportNames")
    assert result_kinds["function_set_builtin"] == "i64"
    assert result_kinds["string_from_bytes"] == "i32"
    assert signatures["function_set_builtin"] == {"params": ["i64"], "result": "i64"}
    assert signatures["string_from_bytes"] == {
        "params": ["i32", "i64", "i32"],
        "result": "i32",
    }
    assert exports == {
        "function_set_builtin": "molt_function_set_builtin",
        "string_from_bytes": "molt_string_from_bytes",
        "runtime_execution_enter": "molt_runtime_execution_enter",
        "runtime_execution_leave": "molt_runtime_execution_leave",
        "runtime_shutdown": "molt_runtime_shutdown",
    }
    assert result_kinds.keys() == signatures.keys() == exports.keys()
    assert (
        'const runtimeExportSignatures = {"function_set_builtin": {"params": ["i64"], "result": "i64"}, "string_from_bytes": {"params": ["i64", "i64", "i64"], "result": "i64"}};'
        in content
    )
    assert "bindRuntimeFunctionImports" in content
    assert "callableTableFromModule(appModule" in content
    assert "callableTableFromModule(runtimeModule" in content
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
    assert "name => runtimeImportExportNames[name] || null," in content
    assert "exportCandidates" not in content
    assert "`molt_${entry.name}`" not in content
    assert (
        "const callSignature = runtimeExportSignatures[entry.name] || signature;"
        in content
    )
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
    assert "name => runtimeExportNames[name] || null," in browser_embed
    assert "exportCandidates" not in browser_embed
    assert "`molt_${entry.name}`" not in browser_embed
    assert (
        "const callSignature = runtimeExportSignatures[entry.name] || signature;"
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
    assert "bindRuntimeFunctionImports," in run_wasm
    assert "let runtimeImportExportNames = runtimeExportByImport;" in run_wasm
    assert (
        "const manifestRuntimeImportExportNames = manifestRuntimeImports.export_names || null;"
        in run_wasm
    )
    assert (
        "runtimeImportExportNames = manifestRuntimeImportExportNames\n"
        "    ? {\n"
        "        ...runtimeExportByImport,\n"
        "        ...manifestRuntimeImportExportNames,\n"
        "      }\n"
        "    : runtimeExportByImport;" in run_wasm
    )
    assert "generatedRuntimeExportByImport" not in run_wasm
    assert run_wasm.count("const runtimeExportNameForImport =") == 1
    assert "const runtimeExportNameForImport = (importName) => {" in run_wasm
    assert "createRuntimeLifetime(runtimeInst, runtimeImportExportNames" in run_wasm
    assert "runtimeLifetime(runtimeInst).execute(operation)" in run_wasm
    assert "exports?.molt_runtime_execution_enter" not in run_wasm
    assert "exports?.molt_runtime_execution_leave" not in run_wasm
    assert "  runtimeExportNameForImport," in run_wasm
    # Reserved callables use the shared dispatcher; direct module imports
    # retain the name-resolution contract below. The dispatcher's argument and
    # result semantics execute in test_wasm_reserved_callable_arity.py.
    assert "return callReservedRuntimeCallable({" in run_wasm
    assert "entry: reservedRuntimeCallable," in run_wasm
    assert "runtimeExportImportSignature(runtimeExport)" in run_wasm
    assert "entry.name.startsWith('molt_')" not in run_wasm
    assert "`molt_${entry.name}`" not in run_wasm
    assert "`molt_${name}`" not in run_wasm


def test_static_wasm_loader_bridge_owns_binary_parser_authority() -> None:
    from pathlib import Path

    root = Path(__file__).resolve().parents[1]
    bridge = (root / "wasm/loader_bridge.js").read_text(encoding="utf-8")
    run_wasm = (root / "wasm/run_wasm.js").read_text(encoding="utf-8")
    assert "const extractWasmTableBase = (buffer) => {" in bridge
    assert "const parseWasmMetadata = (buffer, options = {}) => {" in bridge
    assert "const reservedRuntimeCallablesFromManifest = (manifest) => {" in bridge
    assert "extractWasmTableBase," in bridge
    assert "parseWasmMetadata," in bridge
    assert "parseWasmExportFunctionSignatures," in bridge
    assert "requireWasmCallableTable," in bridge
    assert "verifyCallableTableEntries," in bridge
    assert "reservedRuntimeCallablesFromManifest," in bridge
    assert (
        "const parseWasmImports = (buffer, options = {}) =>\n    parseWasmMetadata(buffer, options).imports;"
        in bridge
    )
    assert (
        "const parseWasmExportFunctionSignatures = (buffer) =>\n    parseWasmMetadata(buffer).exportFunctionSignatures;"
        in bridge
    )
    assert "const outputMetadata = parseWasmMetadata(wasmBuffer, {" in run_wasm
    assert "exportFunctionSignatures: false," in run_wasm
    assert "requireWasmCallableTable(wasmBuffer, 'output wasm')" in run_wasm
    assert "const runtimeMetadata = parseWasmMetadata(runtimeBuffer);" in run_wasm
    assert (
        "const includeExportFunctionSignatures = options.exportFunctionSignatures !== false;"
        in bridge
    )

    consumers = {
        "wasm/browser_host.js": (
            "globalThis.MoltWasmLoaderBridge",
            "extractWasmTableBase,",
            "runtimeCallableTableLayout,",
        ),
        "wasm/run_wasm.js": (
            "require('./loader_bridge.js')",
            "parseWasmMetadata,",
            "parseWasmExportFunctionSignatures: parseWasmExportFunctionSignaturesFromBridge",
            "runtimeCallableTableLayout,",
        ),
        "wasm/browser_embed.js": (
            "globalThis.MoltWasmLoaderBridge",
            "runtimeCallableTableLayoutFromAbi,",
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
        # The reserved-callable layout is generated; a host copy drifted to a
        # stale base (33 vs 30) and misdispatched every split-runtime call.
        "const RESERVED_RUNTIME_CALLABLE_BASE =",
        "const DEFAULT_WASM_APP_TABLE_BASE =",
        "reserved_runtime_callable_base",
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
    assert "normalizeValueForKind," in browser_host
    assert (
        "const loadRuntimeManifest = async (options, moduleUrl = import.meta.url) => {"
        in browser_host
    )
    assert "browser host manifest missing abi.runtime_imports.names" in browser_host
    assert (
        "createRuntimeLifetime(runtimeInstance, runtimeImportAbi?.export_names"
        in browser_host
    )
    assert (
        "runtimeLifetime(runtimeInstance, runtimeImportAbi).execute(operation)"
        in browser_host
    )
    assert "exports?.molt_runtime_execution_enter" not in browser_host
    assert "exports?.molt_runtime_execution_leave" not in browser_host
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
    assert "return bindRuntimeFunctionImports(" in browser_host
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
    assert "name => runtimeExportNames[name] || null" in browser_host
    assert "exportCandidates" not in browser_host
    assert "`molt_${entry.name}`" not in browser_host
    assert (
        "const callSignature = runtimeExportSignatures[entry.name] || signature;"
        in browser_host
    )
    assert "return bindRuntimeFunctionImports(" in browser_host
    assert (
        "normalizeValueForKind(value, callSignature.params[index] || null)"
        in browser_host
    )
    assert "return normalizeValueForKind(fn(...callArgs), resultKind);" in browser_host
    assert "runtimeImportAbi," in browser_host
    assert "bindRuntimeFunctionImports," in browser_host
    assert "entry.name === 'fast_list_append'" not in browser_host
    assert "entry.name === 'dict_setitem'" not in browser_host


def test_effective_split_worker_table_base_uses_backend_authority() -> None:
    from molt.cli.wasm import _effective_split_worker_table_base

    assert (
        _effective_split_worker_table_base(
            wasm_table_base=4096,
            app_callable_table_slots=[4096, 4189],
        )
        == 4096
    )


def test_effective_split_worker_table_base_does_not_infer_fallback() -> None:
    import pytest

    from molt.cli.wasm import _effective_split_worker_table_base

    with pytest.raises(ValueError, match="requires the bound backend table base"):
        _effective_split_worker_table_base(
            wasm_table_base=None,
            app_callable_table_slots=[4130],
        )


def test_effective_split_worker_table_base_rejects_callable_below_backend_base() -> (
    None
):
    import pytest

    from molt.cli.wasm import _effective_split_worker_table_base

    with pytest.raises(ValueError, match="above finalized callable-table slot"):
        _effective_split_worker_table_base(
            wasm_table_base=4096,
            app_callable_table_slots=[4095],
        )


def test_effective_split_worker_table_base_rejects_final_slot_below_backend_base() -> (
    None
):
    import pytest

    from molt.cli.wasm import _effective_split_worker_table_base

    with pytest.raises(ValueError, match="above finalized callable-table slot"):
        _effective_split_worker_table_base(
            wasm_table_base=4096,
            app_callable_table_slots=[1024],
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
            _wasm_function_export("callable_7", 3),
            _wasm_function_export("callable_8", 4),
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
        "socket_drop": "i64",
        "function_set_builtin": "i64",
        "molt_abc_bootstrap": "i64",
        "molt_importlib_import_transaction": "i64",
        "molt_socket_drop": "i64",
        "molt_add": "i64",
        "molt_bool_from_i32": "i64",
        "molt_buffer_acquire": "i32",
        "molt_string_from_bytes": "i32",
        "string_from_bytes": "i32",
    }
    assert _runtime_import_signatures_from_manifest(import_names) == {
        "abc_bootstrap": {"params": [], "result": "i64"},
        "socket_drop": {"params": ["i64"], "result": "i64"},
        "function_set_builtin": {"params": ["i64"], "result": "i64"},
        "molt_abc_bootstrap": {"params": [], "result": "i64"},
        "molt_importlib_import_transaction": {
            "params": ["i64", "i64", "i64", "i64", "i64"],
            "result": "i64",
        },
        "molt_socket_drop": {"params": ["i64"], "result": "i64"},
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
        "PyArg_ParseTuple": "molt_PyArg_ParseTuple",
        "PyFloat_Check": "molt_PyFloat_Check",
    }
    assert _runtime_import_result_kinds_from_manifest(
        {"molt_PyArg_ParseTuple"},
        runtime_export_signatures={
            "molt_PyArg_ParseTuple": {
                "params": ["i32", "i32", "i32"],
                "result": "i32",
            },
        },
    ) == {"molt_PyArg_ParseTuple": "i32"}
    assert _runtime_import_signatures_from_manifest(
        {"molt_PyArg_ParseTuple"},
        runtime_export_signatures={
            "molt_PyArg_ParseTuple": {
                "params": ["i32", "i32", "i32"],
                "result": "i32",
            },
        },
    ) == {
        "molt_PyArg_ParseTuple": {
            "params": ["i32", "i32", "i32"],
            "result": "i32",
        }
    }
    assert _runtime_import_export_names_from_manifest({"molt_PyArg_ParseTuple"}) == {
        "molt_PyArg_ParseTuple": "molt_PyArg_ParseTuple"
    }

    worker_js = _generate_split_worker_js(
        deployment_manifest=split_wasm_execution_manifest(
            EMPTY_WASM_MODULE, EMPTY_WASM_MODULE
        ),
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
    assert '"PyArg_ParseTuple": "molt_PyArg_ParseTuple"' in worker_js
    assert "exportCandidates" not in worker_js
    assert "`molt_${entry.name}`" not in worker_js
    public_worker_js = _generate_split_worker_js(
        deployment_manifest=split_wasm_execution_manifest(
            EMPTY_WASM_MODULE, EMPTY_WASM_MODULE
        ),
        shared_memory_initial_pages=1,
        shared_table_initial=8192,
        shared_table_base=None,
        runtime_import_names={"molt_PyArg_ParseTuple"},
        runtime_export_signatures={
            "molt_PyArg_ParseTuple": {
                "params": ["i32", "i32", "i32"],
                "result": "i32",
            },
        },
    )
    assert '"molt_PyArg_ParseTuple": "molt_PyArg_ParseTuple"' in public_worker_js
    assert 'importName.startsWith("molt_")' not in public_worker_js


def test_runtime_export_signatures_use_cpython_abi_split_export_names(
    monkeypatch,
) -> None:
    import molt.cli.non_native_output as non_native_output

    requested: dict[str, set[str]] = {}

    def fake_export_signatures(runtime_wasm, *, export_names):
        requested["export_names"] = set(export_names)
        return {
            "molt_PyArg_ParseTuple": {
                "params": ["i32", "i32", "i32"],
                "result": "i32",
            },
            "molt_socket_drop": {"params": ["i64"], "result": "i64"},
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
        "socket_drop": {"params": ["i64"], "result": "i64"},
    }
    assert requested["export_names"] == {"molt_PyArg_ParseTuple", "molt_socket_drop"}
    assert non_native_output._runtime_export_signatures_for_imports(
        Path("runtime.wasm"),
        {"molt_PyArg_ParseTuple"},
    ) == {
        "molt_PyArg_ParseTuple": {
            "params": ["i32", "i32", "i32"],
            "result": "i32",
        },
    }
    assert requested["export_names"] == {"molt_PyArg_ParseTuple"}


def test_browser_runtime_manifest_projects_generated_host_publication_roots() -> None:
    from molt.cli.non_native_output import _runtime_host_abi_import_names

    names = _runtime_host_abi_import_names()
    assert {
        "index",
        "len",
        "runtime_execution_enter",
        "runtime_execution_leave",
        "runtime_shutdown",
    } <= names
    assert "molt_main" not in names
    assert "memory" not in names


def test_wasm_export_function_signatures_reads_wasm_bytes(tmp_path) -> None:
    from molt.wasm_artifact import _wasm_export_function_signatures

    wasm_path = tmp_path / "runtime.wasm"
    wasm_path.write_bytes(_signature_fixture_wasm())

    assert _wasm_export_function_signatures(wasm_path, export_names={"callable_8"}) == {
        "callable_8": {
            "params": ["i32", "i64", "i32"],
            "result": "i32",
        },
    }


def test_cli_does_not_expose_table_ref_export_synthesis() -> None:
    import molt.cli as cli

    assert not hasattr(cli, "_export_wasm_table_refs")


def _worker_errno_fixture_wasm() -> bytes:
    """Three real imports with independently specified protocol observations."""
    import molt.wasm_artifact as wasm_artifact

    types = _wasm_vec(
        [
            _wasm_function_type([0x7F, 0x7F], [0x7F]),
            _wasm_function_type([0x7F, 0x7E], [0x7F]),
            _wasm_function_type([0x7E, 0x7F, 0x7F], [0x7F]),
            _wasm_function_type([], [0x7F]),
        ]
    )
    imports = _wasm_vec(
        [
            _wasm_function_import("wasi_snapshot_preview1", "fd_prestat_get", 0),
            _wasm_function_import("wasi_snapshot_preview1", "fd_filestat_set_size", 1),
            _wasm_function_import("env", "molt_socket_connect_host", 2),
        ]
    )
    bodies = [
        b"\x00\x41\x00\x41\x10\x10\x00\x0b",
        b"\x00\x41\x00\x42\x00\x10\x01\x0b",
        b"\x00\x42\x7f\x41\x10\x41\x00\x10\x02\x0b",
    ]
    return wasm_artifact._build_wasm_sections(
        [
            (1, types),
            (2, imports),
            (3, _wasm_vec([wasm_artifact._write_wasm_varuint(3)] * 3)),
            (
                7,
                _wasm_vec(
                    [
                        _wasm_function_export("raw_badf", 3),
                        _wasm_function_export("raw_unsupported", 4),
                        _wasm_function_export("molt_unsupported", 5),
                    ]
                ),
            ),
            (
                10,
                _wasm_vec(
                    [
                        wasm_artifact._write_wasm_varuint(len(body)) + body
                        for body in bodies
                    ]
                ),
            ),
        ]
    )


def _worker_environment_fixture_wasm() -> bytes:
    """WASM calls preview1 and exports the actual memory written by the host."""
    import molt.wasm_artifact as wasm_artifact

    bodies = [
        b"\x00\x41\x00\x41\x04\x10\x00\x0b",
        b"\x00\x41\x10\x41\x80\x20\x10\x01\x0b",
    ]
    return wasm_artifact._build_wasm_sections(
        [
            (
                1,
                _wasm_vec(
                    [
                        _wasm_function_type([0x7F, 0x7F], [0x7F]),
                        _wasm_function_type([], [0x7F]),
                    ]
                ),
            ),
            (
                2,
                _wasm_vec(
                    [
                        _wasm_function_import(
                            "wasi_snapshot_preview1", "environ_sizes_get", 0
                        ),
                        _wasm_function_import(
                            "wasi_snapshot_preview1", "environ_get", 0
                        ),
                        wasm_artifact._write_wasm_string("env")
                        + wasm_artifact._write_wasm_string("memory")
                        + b"\x02\x00\x01",
                    ]
                ),
            ),
            (3, _wasm_vec([b"\x01", b"\x01"])),
            (
                7,
                _wasm_vec(
                    [
                        _wasm_function_export("sizes", 2),
                        _wasm_function_export("environment", 3),
                        wasm_artifact._write_wasm_string("memory") + b"\x02\x00",
                    ]
                ),
            ),
            (
                10,
                _wasm_vec(
                    [
                        wasm_artifact._write_wasm_varuint(len(body)) + body
                        for body in bodies
                    ]
                ),
            ),
        ]
    )


def _observe_generated_worker_environment(tmp_path, source, *, mode, deployment):
    """Execute real generated WASI imports through a real guest module."""
    from tests.wasm_callable_table_fixtures import attested_empty_callable_table

    if shutil.which("node") is None:
        pytest.skip("Node is required for generated-worker WASI environment")
    root = Path(__file__).resolve().parents[1]
    module = tmp_path / "environment.wasm"
    module.write_bytes(_worker_environment_fixture_wasm())
    empty = tmp_path / "empty.wasm"
    empty.write_bytes(attested_empty_callable_table(EMPTY_WASM_MODULE))
    manifest = tmp_path / "probe-manifest.json"
    manifest.write_text(json.dumps(deployment), encoding="utf-8")
    worker = tmp_path / "probe-worker.js"
    worker.write_text(source, encoding="utf-8")
    script = tmp_path / "worker-environment.cjs"
    script.write_text(
        r"""
const assert = require('node:assert/strict');
const fs = require('node:fs');
const path = require('node:path');
const vm = require('node:vm');
const [workerPath, modulePath, manifestPath, mode, root, emptyPath] = process.argv.slice(2);
let source = fs.readFileSync(workerPath, 'utf8');
const bridge = require(path.join(root, 'wasm/loader_bridge.js'));
const lifetime = require(path.join(root, 'wasm/runtime_lifecycle.js'));
const {MoltVfs} = require(path.join(root, 'wasm/molt_vfs_browser.js'));
const empty = new WebAssembly.Module(fs.readFileSync(emptyPath));
const context = vm.createContext({WebAssembly, TextEncoder, TextDecoder, console,
  URL, Response, performance, MoltWasmLoaderBridge: bridge,
  MoltRuntimeLifecycle: lifetime, MoltVfs, runtimeModule: empty, appModule: empty});
(async () => {
  let probe;
  if (mode === 'static') {
    const boundary = source.indexOf('export default {');
    assert.ok(boundary > 0);
    vm.runInContext(source.slice(0, boundary) + `
      const state = {memory: new WebAssembly.Memory({initial:1})};
      globalThis.probe = {memory:state.memory, wasi:buildWasiShim(state,new StdioCapture()),
        admitted:deploymentManifest};`, context);
    probe = context.probe;
  } else {
    const boundary = '    // Shared table for indirect calls';
    assert.ok(source.includes(boundary));
    source = source.replace(/^import .*;$/gm, '')
      .replace('export default {', 'globalThis.worker = {')
      .replace(boundary, '    return {wasi, memory:wasmMemory, admitted:deploymentManifest};\n' + boundary);
    vm.runInContext(source, context);
    probe = await context.worker.fetch({url:'https://worker.example/'}, {SECRET_BINDING:'never guest env'}, {});
  }
  assert.deepEqual(JSON.parse(JSON.stringify(probe.admitted)),
    JSON.parse(fs.readFileSync(manifestPath, 'utf8')));
  const wasmModule = new WebAssembly.Module(fs.readFileSync(modulePath));
  const instance = new WebAssembly.Instance(wasmModule, {
    env:{memory:probe.memory}, wasi_snapshot_preview1:probe.wasi});
  assert.equal(instance.exports.sizes(), 0);
  assert.equal(instance.exports.environment(), 0);
  const view = new DataView(instance.exports.memory.buffer);
  const count = view.getUint32(0, true), size = view.getUint32(4, true);
  const entries = [];
  let observedSize = 0;
  for (let i = 0; i < count; i++) {
    const pointer = view.getUint32(16 + i * 4, true);
    assert.equal(pointer, 4096 + observedSize, 'preview1 environment pointer');
    const bytes = new Uint8Array(instance.exports.memory.buffer, pointer);
    const end = bytes.indexOf(0);
    assert.ok(end >= 0, 'environment entry is NUL terminated');
    entries.push(new TextDecoder().decode(bytes.subarray(0, end)));
    observedSize += end + 1;
  }
  assert.equal(size, observedSize, 'preview1 reports UTF-8 bytes including NULs');
  const pairs = entries.map(entry => {
    const separator = entry.indexOf('=');
    assert.ok(separator > 0);
    return [entry.slice(0,separator), entry.slice(separator+1)];
  });
  assert.equal(new Set(pairs.map(pair => pair[0])).size, count, 'environment names are unique');
  console.log(JSON.stringify(Object.fromEntries(pairs)));
})().catch(error => { console.error(error); process.exitCode=1; });
""",
        encoding="utf-8",
    )
    result = run_guarded_test_process(
        [
            "node",
            str(script),
            str(worker),
            str(module),
            str(manifest),
            mode,
            str(root),
            str(empty),
        ],
        cwd=tmp_path,
        timeout=30,
    )
    assert result.returncode == 0, result.stderr
    assert result.descendants_closed
    return json.loads(result.stdout)


@pytest.mark.parametrize("mode", ["static", "split"])
@pytest.mark.parametrize("grant", ["", "fs.read"])
def test_generated_worker_delivers_manifest_policy_through_real_wasi(
    tmp_path, grant, mode
):
    from molt.capability_manifest import CapabilityManifest
    from molt.cli.wasm import _generate_split_worker_js
    from tests.wasm_callable_table_fixtures import attested_empty_callable_table
    from tools.generate_worker import generate_worker

    module = tmp_path / "environment.wasm"
    module.write_bytes(_worker_environment_fixture_wasm())
    policy = CapabilityManifest(allow=[grant] if grant else []).resolve()
    if mode == "static":
        manifest = write_wasm_execution_manifest(tmp_path, linked=module, policy=policy)
        deployment = json.loads(manifest.read_text(encoding="utf-8"))
    else:
        empty = attested_empty_callable_table(EMPTY_WASM_MODULE)
        deployment = split_wasm_execution_manifest(empty, empty, policy=policy)
    deployment["fixture_metadata"] = {
        "future_field": "{{TMP_QUOTA_MB}} __MOLT_SHARED_TABLE_BASE__"
    }
    deployment["launch"]["environment"]["FIXTURE_UNICODE"] = "héllo=世界"
    deployment["launch"]["environment"]["FIXTURE_TOKEN"] = (
        "{{WASM_FILENAME}} __MOLT_SHARED_TABLE_BASE__"
    )
    deployment["launch"]["environment"]["__proto__"] = "ordinary data"
    deployment["launch"]["environment"]["MOLT_EXECUTION_TARGET"] = "native"
    if mode == "static":
        manifest.write_text(json.dumps(deployment), encoding="utf-8")
        worker = tmp_path / "worker.js"
        generate_worker(worker)
        source = worker.read_text(encoding="utf-8")
    else:
        source = _generate_split_worker_js(
            deployment_manifest=deployment,
            shared_memory_initial_pages=1,
            shared_table_initial=1,
            shared_table_base=32,
        )
    observed = _observe_generated_worker_environment(
        tmp_path, source, mode=mode, deployment=deployment
    )
    expected = {
        "FIXTURE_TOKEN": "{{WASM_FILENAME}} __MOLT_SHARED_TABLE_BASE__",
        "FIXTURE_UNICODE": "héllo=世界",
        "MOLT_CAPABILITIES": grant,
        "MOLT_CAPABILITY_POLICY_DIGEST": policy.digest(),
        "MOLT_CAPABILITY_TIER": "none",
        "MOLT_EXECUTION_TARGET": "cloudflare",
        "__proto__": "ordinary data",
    }
    if mode == "split":
        expected["MOLT_WASM_TABLE_BASE"] = "32"
    assert observed == expected


def _worker_fetch_fixture_wasm() -> bytes:
    """Real guest startup and shutdown write an ordered trace through fd_write."""
    import molt.wasm_artifact as wasm_artifact
    from tests.wasm_callable_table_fixtures import attested_empty_callable_table

    messages = [b"enter\n", b"worker-ok\n", b"status\n", b"leave\n", b"shutdown\n"]
    offsets = [16 + sum(map(len, messages[:index])) for index in range(len(messages))]

    def body(before: int, after: int, message: int, result: int | None = None) -> bytes:
        # All i32 immediates fit one signed LEB byte in this bounded fixture.
        assert offsets[message] < 64 and len(messages[message]) < 64
        code = b"\x23\x00\x41" + bytes([before]) + b"\x47\x04\x40\x00\x0b"
        code += b"\x41" + bytes([after]) + b"\x24\x00"
        code += b"\x41" + bytes([offsets[message]])
        code += b"\x41" + bytes([len(messages[message])]) + b"\x10\x01"
        if result is not None:
            code += b"\x42" + bytes([result])
        return b"\x00" + code + b"\x0b"

    bodies = [
        # print(ptr,len): publish one iovec, call the actual WASI fd_write,
        # and trap if the host reports failure.
        b"\x00\x41\x00\x20\x00\x36\x02\x00"
        b"\x41\x04\x20\x01\x36\x02\x00"
        b"\x41\x01\x41\x00\x41\x01\x41\x08\x10\x00\x04\x40\x00\x0b\x0b",
        body(0, 1, 0, 1),
        body(1, 2, 1),
        body(2, 2, 2, 0),
        body(2, 3, 3),
        body(3, 4, 4, 1),
        b"\x00\x42\x00\x0b",
    ]
    types = [
        _wasm_function_type([0x7F] * 4, [0x7F]),
        _wasm_function_type([], [0x7E]),
        _wasm_function_type([0x7E], []),
        _wasm_function_type([], []),
        _wasm_function_type([0x7F, 0x7F], []),
    ]
    exports = [
        wasm_artifact._write_wasm_string("memory") + b"\x02\x00",
        *[
            _wasm_function_export(name, index)
            for name, index in [
                ("molt_runtime_execution_enter", 2),
                ("molt_main", 3),
                ("molt_main_exit_status", 4),
                ("molt_runtime_execution_leave", 5),
                ("molt_runtime_shutdown", 6),
                ("molt_exception_pending", 7),
            ]
        ],
    ]
    payload = b"".join(messages)
    return attested_empty_callable_table(
        wasm_artifact._build_wasm_sections(
            [
                (1, _wasm_vec(types)),
                (
                    2,
                    _wasm_vec(
                        [_wasm_function_import("wasi_snapshot_preview1", "fd_write", 0)]
                    ),
                ),
                (3, _wasm_vec([bytes([kind]) for kind in [4, 1, 3, 1, 2, 1, 1]])),
                (5, b"\x01\x00\x01"),
                (6, b"\x01\x7f\x01\x41\x00\x0b"),
                (7, _wasm_vec(exports)),
                (
                    10,
                    _wasm_vec(
                        [
                            wasm_artifact._write_wasm_varuint(len(code)) + code
                            for code in bodies
                        ]
                    ),
                ),
                (
                    11,
                    b"\x01\x00\x41\x10\x0b"
                    + wasm_artifact._write_wasm_varuint(len(payload))
                    + payload,
                ),
            ]
        )
    )


def test_generated_worker_fetch_uses_real_module_instance_and_cleans_up(
    tmp_path: Path,
) -> None:
    from tools.generate_worker import generate_worker

    module = tmp_path / "worker_linked.wasm"
    module.write_bytes(_worker_fetch_fixture_wasm())
    write_wasm_execution_manifest(tmp_path, linked=module)
    output = tmp_path / "worker.js"
    generate_worker(output)
    source = output.read_text(encoding="utf-8")
    assert source.count("export default {") == 1
    script = tmp_path / "worker-fetch.cjs"
    script.write_text(
        "const workerSource = "
        + json.dumps(source.replace("export default {", "globalThis.worker = {", 1))
        + ";\nconst modulePath = "
        + json.dumps(str(module))
        + ";\n"
        + r"""
const assert = require('node:assert/strict');
const fs = require('node:fs');
const vm = require('node:vm');
const {webcrypto} = require('node:crypto');
const bytes = fs.readFileSync(modulePath);
const buffer = bytes.buffer.slice(bytes.byteOffset, bytes.byteOffset + bytes.byteLength);
const errors = [];
const originalCompile = WebAssembly.compile;
let compiles = 0;
WebAssembly.compile = async bytes => {
  compiles++;
  return originalCompile.call(WebAssembly, bytes);
};
const context = vm.createContext({WebAssembly, ArrayBuffer, Uint8Array, DataView,
  TextEncoder, TextDecoder, Response, URL, performance, crypto: webcrypto,
  console: {error: (...args) => errors.push(args)}});
vm.runInContext(workerSource, context);
(async () => {
  try {
    const response = await context.worker.fetch(new Request('https://worker.example/route'), {
      __STATIC_CONTENT: {get: async (name, options) => {
        assert.equal(name, 'worker_linked.wasm');
        assert.equal(options.type, 'arrayBuffer');
        return buffer;
      }},
    }, {});
    assert.equal(response.status, 200, JSON.stringify(errors));
    assert.equal(await response.text(), 'enter\nworker-ok\nstatus\nleave\nshutdown\n');
    assert.equal(compiles, 1);
    assert.equal(errors.length, 0);
    const corrupt = buffer.slice(0);
    new Uint8Array(corrupt)[corrupt.byteLength - 1] ^= 1;
    const refused = await context.worker.fetch(new Request('https://worker.example/route'), {
      __STATIC_CONTENT: {get: async () => corrupt},
    }, {});
    assert.equal(refused.status, 500);
    assert.equal(await refused.text(), 'internal error');
    assert.equal(compiles, 1, 'corrupted bytes reached WebAssembly.compile');
    assert.match(String(errors.at(-1)), /SHA-256 mismatch/);
    console.log('actual worker fetch, guest startup, runtime cleanup, byte refusal');
  } finally { WebAssembly.compile = originalCompile; }
})().catch(error => { console.error(error); process.exitCode = 1; });
""",
        encoding="utf-8",
    )
    result = run_guarded_test_process(["node", str(script)], cwd=tmp_path, timeout=30)
    assert result.returncode == 0, result.stderr
    assert result.descendants_closed
    assert result.stdout.strip() == (
        "actual worker fetch, guest startup, runtime cleanup, byte refusal"
    )


def test_generated_workers_keep_actual_wasi_and_molt_error_imports_separate(
    tmp_path: Path,
) -> None:
    import pytest
    from molt.cli.wasm import _generate_split_worker_js
    from tests.wasm_callable_table_fixtures import attested_empty_callable_table
    from tools.generate_worker import generate_worker

    if shutil.which("node") is None:
        pytest.skip("Node is required for actual generated-worker host imports")
    root = Path(__file__).resolve().parents[1]
    fixture = tmp_path / "errno-probe.wasm"
    fixture.write_bytes(_worker_errno_fixture_wasm())
    empty = tmp_path / "empty-attested.wasm"
    empty.write_bytes(attested_empty_callable_table(EMPTY_WASM_MODULE))
    write_wasm_execution_manifest(tmp_path, linked=fixture)
    static = tmp_path / "static-worker.js"
    generate_worker(static)
    split = tmp_path / "split-worker.js"
    split.write_text(
        _generate_split_worker_js(
            deployment_manifest=split_wasm_execution_manifest(
                empty.read_bytes(), empty.read_bytes()
            ),
            shared_memory_initial_pages=8,
            shared_table_initial=16,
            shared_table_base=None,
        ),
        encoding="utf-8",
    )
    script = tmp_path / "worker-errno.cjs"
    script.write_text(
        r"""
const assert = require('node:assert/strict');
const fs = require('node:fs');
const path = require('node:path');
const vm = require('node:vm');
const [root, staticPath, splitPath, fixturePath, emptyPath] = process.argv.slice(2);
const fixture = new WebAssembly.Module(fs.readFileSync(fixturePath));
const empty = new WebAssembly.Module(fs.readFileSync(emptyPath));
const bridge = require(path.join(root, 'wasm/loader_bridge.js'));
const lifetime = require(path.join(root, 'wasm/runtime_lifecycle.js'));
const {MoltVfs} = require(path.join(root, 'wasm/molt_vfs_browser.js'));
async function observe(source, kind) {
  const context = vm.createContext({
    WebAssembly, TextEncoder, TextDecoder, URL, Response, console, performance,
    MoltWasmLoaderBridge: bridge, MoltRuntimeLifecycle: lifetime, MoltVfs,
    runtimeModule: empty, appModule: empty,
  });
  if (kind === 'static') {
    // Keep the actual host/WASI factories; the request/runtime lifetime is not under test.
    const boundary = source.indexOf('export default {');
    assert.ok(boundary > 0);
    vm.runInContext(source.slice(0, boundary) + `
      const state = {memory:new WebAssembly.Memory({initial:1})};
      const tmp = new TmpFs(1024); const stdio = new StdioCapture();
      globalThis.probeImports = {env:createHostImports(state,tmp,stdio),
        wasi_snapshot_preview1:buildWasiShim(state,stdio)};`, context);
  } else {
    // Execute the real request-owned import construction, stopping before runtime instantiation.
    const boundary = '    // Shared table for indirect calls';
    assert.ok(source.includes(boundary));
    source = source.replace(/^import .*;$/gm, '')
      .replace('export default {', 'globalThis.worker = {')
      .replace(boundary, '    return {env:hostEnv,wasi_snapshot_preview1:wasi};\n' + boundary);
    vm.runInContext(source, context);
    context.probeImports = await context.worker.fetch({url:'https://worker.example/'}, {}, {});
  }
  const instance = new WebAssembly.Instance(fixture, context.probeImports);
  return [instance.exports.raw_badf(), instance.exports.raw_unsupported(),
    instance.exports.molt_unsupported()];
}
(async () => {
  for (const [kind, filename] of [['static', staticPath], ['split', splitPath]]) {
    const source = fs.readFileSync(filename, 'utf8');
    assert.deepEqual(await observe(source, kind), [8, 52, -38], kind);
    // A plausible cross-domain substitution must fail the same independent oracle.
    const crossed = source.replace('= GUEST_ERRNO;', '= WASI_ERRNO;');
    assert.notEqual(crossed, source);
    assert.notDeepEqual(await observe(crossed, kind), [8, 52, -38], `${kind} crossed domains`);
  }
  console.log('actual generated-worker error domains');
})().catch(error => { console.error(error); process.exitCode=1; });
""",
        encoding="utf-8",
    )
    result = run_guarded_test_process(
        [
            "node",
            str(script),
            str(root),
            str(static),
            str(split),
            str(fixture),
            str(empty),
        ],
        cwd=root,
        timeout=30,
    )
    assert result.returncode == 0, result.stderr
    assert result.descendants_closed
    assert result.stdout.strip() == "actual generated-worker error domains"
