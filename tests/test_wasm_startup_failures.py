"""Execute shipped JS startup consumers without compiling a runtime artifact."""

from __future__ import annotations

import json
import shutil
from pathlib import Path

import pytest

from tests.process_guard_common import run_guarded_test_process


ROOT = Path(__file__).resolve().parents[1]


def _run_node(script: str, tmp_path: Path) -> None:
    node = shutil.which("node")
    if node is None:
        pytest.skip("Node is required for JavaScript startup consumer proof")
    path = tmp_path / "startup-contract.cjs"
    path.write_text(script, encoding="utf-8")
    run = run_guarded_test_process(
        [node, str(path)],
        cwd=ROOT,
        text=True,
        capture_output=True,
        timeout=30,
        check=False,
    )
    assert run.returncode == 0, run.stderr


def test_node_runner_rejects_startup_error_before_host_exports(tmp_path: Path) -> None:
    source = (ROOT / "wasm/run_wasm.js").read_text(encoding="utf-8")
    start = source.index("const runMain = async () => {")
    end = source.index("\nlet terminationHandlersInstalled", start)
    run_main = source[start:end]
    _run_node(
        "const runMainSource = "
        + json.dumps(run_main)
        + ";\n"
        + r"""
const vm = require('node:vm');
const assert = require('node:assert/strict');

async function exercise(failure, split) {
  const events = [];
  const runtime = {};
  const context = {
    appInstanceForHostCalls: null, appInstanceForExceptions: null,
    runtimeInstance: runtime, initWasmAssets() {}, traceMark() {}, traceRun: false,
    wasmBuffer: {}, linkedBuffer: split ? null : {}, runtimeBuffer: {},
    runtimeManifest: {mode: split ? 'split-runtime' : 'linked'},
    outputImports: null, inputHasRuntimeImports: false, runtimeImportsDesc: null,
    runtimeCallIndirectNames: [], runtimeExportSignatures: {}, canDirectLink: false,
    loadRuntimeAssets() {},
    parseWasmMetadata() {
      return {imports: {funcImports: split ? [{module: 'molt_runtime'}] : [],
                        table: true, memory: true}, exportFunctionSignatures: {}};
    },
    runLinked: async () => events.push('startup'),
    runDirectLink: async () => events.push('startup'),
    runHostExportCalls: async () => events.push('host-export'),
    pendingRuntimeExceptionMessage(instance) { return instance === runtime ? failure : null; },
    wasiExitCode: null, maybeDumpRuntimeProfile() {},
    shutdownHostWorkers: async () => events.push('shutdown'),
  };
  vm.createContext(context);
  vm.runInContext(runMainSource + '\nthis.invoke = runMain;', context);
  if (failure) {
    await assert.rejects(context.invoke(), error => error.message === failure);
    assert.deepEqual(events, ['startup', 'shutdown']);
  } else {
    assert.equal(await context.invoke(), 0);
    assert.deepEqual(events, ['startup', 'host-export', 'shutdown']);
  }
}
(async () => {
  for (const split of [false, true]) {
    await exercise('RuntimeError: original bootstrap diagnostic', split);
    await exercise(null, split);
  }
})().catch(error => { console.error(error); process.exitCode = 1; });
""",
        tmp_path,
    )


@pytest.mark.parametrize("host", ["browser_host", "browser_embed"])
def test_browser_host_initialization_requires_one_real_host_init(
    tmp_path: Path, host: str
) -> None:
    source = (ROOT / f"wasm/{host}.js").read_text(encoding="utf-8")
    if host == "browser_host":
        start = source.index("  let hostExportsInitialized = false;")
        end = source.index("  const makeExportInvoker =", start)
        initializer = (
            source[start:end]
            + "\nthis.initialize = () => ensureHostExportsInitialized(app);"
        )
    else:
        start = source.index("  let initialized = false;")
        end = source.index("  const callBytes =", start)
        initializer = source[start:end] + "\nthis.initialize = ensureInitialized;"
    _run_node(
        "const initializer = "
        + json.dumps(initializer)
        + ";\n"
        + "const browserHost = "
        + json.dumps(host == "browser_host")
        + ";\n"
        + r"""
const vm = require('node:vm');
const assert = require('node:assert/strict');
for (const status of ['missing', 'pending', 'ok']) {
  let calls = 0;
  const app = {exports: {
    molt_isolate_bootstrap() { throw new Error('raw bootstrap fallback invoked'); },
  }};
  if (status !== 'missing') app.exports.molt_host_init = () => { calls++; };
  const context = {
    app, state: {runtimeInstance: {}, memory: {}},
    embed: {appInstance: app, runtimeInstance: {}, memory: {}},
    runtimeExceptionPending: () => status === 'pending',
    pendingRuntimeExceptionMessage: () => status === 'pending' ? 'original failure' : null,
  };
  vm.createContext(context);
  vm.runInContext(initializer, context);
  if (status === 'missing') {
    assert.throws(context.initialize, /molt_host_init export missing/);
    assert.equal(calls, 0);
  } else if (status === 'pending') {
    assert.throws(context.initialize, /original failure/);
    assert.throws(context.initialize, /original failure/);
    assert.equal(calls, 0, 'pending state must prevent initialization');
  } else {
    context.initialize();
    context.initialize();
    assert.equal(calls, 1);
  }
}
""",
        tmp_path,
    )


@pytest.mark.parametrize("host", ["run_wasm", "browser_host"])
def test_status_abi_is_validated_before_application_execution(
    tmp_path: Path, host: str
) -> None:
    source = (ROOT / f"wasm/{host}.js").read_text(encoding="utf-8")
    start = source.index("const withRuntimeExecution =")
    delimiter = (
        "const withOwnedValue ="
        if host == "run_wasm"
        else "let browserVfsModulePromise"
    )
    end = source.index(delimiter, start)
    boundary = source[start:end]
    _run_node(
        "const boundary = "
        + json.dumps(boundary)
        + ";\n"
        + "const browser = "
        + json.dumps(host == "browser_host")
        + ";\n"
        + "const bridgePath = "
        + json.dumps(str(ROOT / "wasm/loader_bridge.js"))
        + ";\n"
        + r"""
const vm = require('node:vm');
const assert = require('node:assert/strict');
const {runtimeExceptionPending} = require(bridgePath);
const names = {runtime_execution_enter: 'enter', runtime_execution_leave: 'leave'};
for (const status of [undefined, 0, () => 0, ignored => 0n, () => 2n, () => 0n, () => 1n]) {
  const events = [];
  const runtime = {exports: {
    enter() { events.push('enter'); return 1n; },
    leave() { events.push('leave'); },
    ...(status === undefined ? {} : {molt_exception_pending: status}),
  }};
  const context = {runtimeExceptionPending, runtimeImportExportNames: names,
    pendingRuntimeExceptionMessage: () => 'original pending error'};
  vm.createContext(context);
  vm.runInContext(boundary + '\nthis.invoke = withRuntimeExecution;', context);
  const operation = () => { events.push('application'); };
  const invoke = () => browser
    ? context.invoke(runtime, {export_names: names}, operation)
    : context.invoke(runtime, operation);
  if (typeof status === 'function' && status.length === 0 &&
      status() === 0n) {
    invoke();
    assert.deepEqual(events, ['enter', 'application', 'leave']);
  } else {
    assert.throws(invoke, /molt_exception_pending|pending.*(runtime exception|error)/);
    assert.deepEqual(events, ['enter', 'leave']);
  }
}
""",
        tmp_path,
    )


@pytest.mark.parametrize("host", ["run_wasm", "browser_host", "browser_embed"])
def test_pending_status_without_diagnostic_object_is_never_success(
    tmp_path: Path, host: str
) -> None:
    source = (ROOT / f"wasm/{host}.js").read_text(encoding="utf-8")
    start = source.index("const pendingRuntimeExceptionMessage =")
    end = source.index("\n};", start) + 3
    formatter = source[start:end]
    _run_node(
        "const formatter = "
        + json.dumps(formatter)
        + ";\n"
        + "const bridgePath = "
        + json.dumps(str(ROOT / "wasm/loader_bridge.js"))
        + ";\n"
        + r"""
const vm = require('node:vm');
const assert = require('node:assert/strict');
const {runtimeExceptionPending} = require(bridgePath);
const runtime = {exports: {molt_exception_pending: () => 1n}};
const context = {runtimeExceptionPending};
vm.createContext(context);
vm.runInContext(formatter + '\nthis.format = pendingRuntimeExceptionMessage;', context);
assert.match(context.format(runtime, null), /Unhandled Molt exception/);
assert.equal(runtime.exports.molt_exception_pending(), 1n);
""",
        tmp_path,
    )


def test_generated_linked_worker_fails_closed_on_pending_or_missing_status(
    tmp_path: Path,
) -> None:
    from tools.generate_worker import generate_worker

    output = tmp_path / "worker.js"
    generate_worker(output, ["fs.bundle.read"], tmp_quota_mb=32)
    source = output.read_text(encoding="utf-8")
    assert source.count("export default {") == 1
    source = source.replace("export default {", "globalThis.worker = {", 1)
    _run_node(
        "const workerSource = "
        + json.dumps(source)
        + ";\n"
        + r"""
const vm = require('node:vm');
const assert = require('node:assert/strict');

async function exercise(status) {
  const events = [];
  let pending = 0n;
  const exports = {
    molt_main() { events.push('main'); pending = status === 'pending' ? 1n : 0n; },
  };
  if (status !== 'missing') exports.molt_exception_pending = () => pending;
  const context = {
    TextDecoder, TextEncoder, Uint8Array, DataView, Response, performance,
    console: {error() {}},
    WebAssembly: {
      compile: async x => x,
      instantiate: async () => ({instance: {exports}}),
    },
  };
  vm.createContext(context);
  vm.runInContext(workerSource, context);
  const response = await context.worker.fetch(
    {body: null}, {__STATIC_CONTENT: {get: async () => new Uint8Array()}}, {},
  );
  assert.equal(response.status, status === 'ok' ? 200 : 500);
  assert.deepEqual(events, status === 'missing' ? [] : ['main']);
  assert.equal(pending, status === 'pending' ? 1n : 0n,
               'host must not clear pending diagnostic state');
}
(async () => {
  for (const status of ['missing', 'pending', 'ok']) await exercise(status);
})().catch(error => { console.error(error); process.exitCode = 1; });
""",
        tmp_path,
    )
