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


def test_host_stream_constructors_box_capacity_and_retire_unpublished_handles(
    tmp_path: Path,
) -> None:
    node_source = (ROOT / "wasm/run_wasm.js").read_text(encoding="utf-8")
    browser_source = (ROOT / "wasm/browser_host.js").read_text(encoding="utf-8")
    assert node_source.count("createRuntimeStream(runtimeInstance, 0n)") == 3
    assert browser_source.count("createRuntimeStream(runtime, 0n)") == 1
    for source in (node_source, browser_source):
        assert ".molt_stream_new(0n)" not in source
    _run_node(
        "const lifecyclePath = "
        + json.dumps(str(ROOT / "wasm/runtime_lifecycle.js"))
        + ";\n"
        + r"""
const assert = require('node:assert/strict');
const {createRuntimeStream} = require(lifecyclePath);
for (const failure of ['none', 'integer', 'stream', 'cleanup', 'stream-cleanup']) {
  const integers = new Map(), streams = new Set(), events = [];
  let pending = false;
  const instance = {exports: {
    molt_exception_pending_fast: () => pending ? 1n : 0n,
    molt_int_from_i64(value) {
      assert.equal(value, 0n);
      integers.set(123n, value);
      events.push('box');
      if (failure === 'integer') pending = true;
      return 123n;
    },
    molt_dec_ref_obj(bits) {
      assert(integers.delete(bits), 'only the boxed capacity is refcounted');
      events.push('release-capacity');
      if (failure.includes('cleanup')) throw new Error('capacity cleanup failed');
    },
    molt_stream_new(bits) {
      assert.equal(integers.get(bits), 0n);
      assert.notEqual(bits, 0n, 'raw zero must not cross the boxed constructor ABI');
      events.push('construct');
      if (failure.startsWith('stream')) { pending = true; return 999n; }
      streams.add(456n);
      return 456n;
    },
    molt_stream_drop(bits) {
      assert(streams.delete(bits), 'unpublished opaque handles need stream drop');
      events.push('drop-stream');
    },
  }};
  if (failure === 'none') {
    assert.equal(createRuntimeStream(instance, 0n), 456n);
    assert.deepEqual(events, ['box', 'construct', 'release-capacity']);
    instance.exports.molt_stream_drop(456n);
  } else {
    assert.throws(() => createRuntimeStream(instance, 0n), error => {
      if (failure === 'stream-cleanup') {
        assert(error instanceof AggregateError);
        assert.match(error.errors[0].message, /stream allocation failed/);
      }
      return true;
    });
    assert.equal(events.includes('drop-stream'), failure === 'cleanup');
  }
  assert.equal(integers.size, 0);
  assert.equal(streams.size, 0);
}
""",
        tmp_path,
    )


def test_host_list_builders_share_consuming_failure_transaction(tmp_path: Path) -> None:
    for host in ("run_wasm.js", "browser_host.js", "browser_embed.js"):
        source = (ROOT / "wasm" / host).read_text(encoding="utf-8")
        assert "makeRuntimeIntList" in source
        assert "withRuntimeOwnedValues" in source
        assert "const boxInt =" not in source
        assert "molt_list_builder_append(builder" not in source
        assert ".map((value) => Number(value))" not in source
        assert ".map((v) => Number(v))" not in source
    _run_node(
        "const lifecyclePath = "
        + json.dumps(str(ROOT / "wasm/runtime_lifecycle.js"))
        + ";\n"
        + r"""
const assert = require('node:assert/strict');
const {boxRuntimeInt, makeRuntimeIntList, withRuntimeOwnedValues} = require(lifecyclePath);
const wide = [-(1n << 63n), -(1n << 46n) - 1n, -(1n << 46n),
  (1n << 46n) - 1n, 1n << 46n, (1n << 63n) - 1n];
function runtime(failure, cleanupFails = false) {
  const live = new Map(), events = [];
  let next = 1000n, pending = false, appendCount = 0, integerCount = 0;
  const allocate = object => { const bits = next++; live.set(bits, {...object, refs: 1}); return bits; };
  const release = bits => {
    if (bits === 0n) return;
    const value = live.get(bits);
    assert.ok(value, 'owner released twice');
    if (--value.refs === 0) {
      live.delete(bits);
      for (const item of value.items || []) release(item);
    }
  };
  const instance = {exports: {
    molt_int_from_i64(value) {
      events.push(['integer', value]);
      const bits = allocate({value});
      if (failure === 'box2' && ++integerCount === 3) pending = true;
      return bits;
    },
    molt_exception_pending_fast() { return pending ? 1n : 0n; },
    molt_list_builder_new(capacity) {
      assert.equal(live.get(capacity).value, 2n);
      events.push(['new']);
      if (failure === 'new') pending = true;
      return allocate({items: [], builder: true});
    },
    molt_list_builder_append(builder, item) {
      assert.ok(live.get(builder).builder);
      events.push(['append', live.get(item).value]);
      if (failure === 'append' && ++appendCount === 2) { pending = true; return 1; }
      live.get(item).refs++;
      live.get(builder).items.push(item);
      return 0;
    },
    molt_list_builder_finish(builder) {
      events.push(['finish']);
      const value = live.get(builder);
      live.delete(builder);
      if (failure === 'finish') {
        value.items.forEach(release);
        pending = true;
        return 0n;
      }
      return allocate({items: value.items});
    },
    molt_dec_ref_obj(bits) {
      events.push(['drop', bits]);
      const builder = live.get(bits)?.builder;
      release(bits);
      if (cleanupFails && builder) throw new Error('builder cleanup failed');
    },
  }};
  return {instance, live, events};
}
for (const failure of [null, 'new', 'append', 'box2', 'finish']) {
  const {instance, live, events} = runtime(failure);
  let result;
  if (failure) {
    assert.throws(() => makeRuntimeIntList(instance, [wide[0], wide[5]]),
      /allocation failed|append failed|finish failed/);
  } else {
    result = makeRuntimeIntList(instance, [wide[0].toString(), wide[5]]);
    assert.deepEqual(live.get(result).items.map(bits => live.get(bits).value), [wide[0], wide[5]]);
    assert.ok(live.get(result).items.every(bits => live.get(bits).refs === 1));
    instance.exports.molt_dec_ref_obj(result);
  }
  assert.equal(live.size, 0, String(failure));
  if (failure && failure !== 'finish') assert.equal(events.some(([event]) => event === 'finish'), false);
}
{
  const {instance, live} = runtime('append', true);
  assert.throws(() => makeRuntimeIntList(instance, wide.slice(0, 2)), error => {
    assert.ok(error instanceof AggregateError);
    assert.match(error.errors[0].message, /append failed/);
    assert.match(error.errors[1].message, /builder cleanup failed/);
    assert.equal(error.cause, error.errors[0]);
    return true;
  });
  assert.equal(live.size, 0);
}
{
  const {instance, live, events} = runtime();
  for (const value of [...wide, ...wide.map(String), 42, '+42']) {
    const bits = boxRuntimeInt(instance, value);
    assert.equal(live.get(bits).value, BigInt(value));
    instance.exports.molt_dec_ref_obj(bits);
  }
  for (const invalid of [Number.MAX_SAFE_INTEGER + 1, 1.5, NaN, Infinity, null, true,
    {}, '', ' 42', '42 ', '42\n', '0x20', '1.0', '1e3', (1n << 63n), -(1n << 63n) - 1n]) {
    const count = events.length;
    assert.throws(() => boxRuntimeInt(instance, invalid), /host integer/);
    assert.equal(events.length, count, 'invalid input reached runtime');
    assert.throws(() => makeRuntimeIntList(instance, [7n, invalid]), /host integer/);
    assert.equal(events.length, count, 'invalid list input allocated');
  }
  assert.throws(() => withRuntimeOwnedValues(instance, [wide[5], 'invalid'],
    value => boxRuntimeInt(instance, value), () => assert.fail('called')), /host integer/);
  assert.equal(live.size, 0);
  const primary = new Error('invocation failed'), cleanup = new Error('cleanup failed');
  const drop = instance.exports.molt_dec_ref_obj;
  instance.exports.molt_dec_ref_obj = bits => { drop(bits); throw cleanup; };
  assert.throws(() => withRuntimeOwnedValues(instance, wide.slice(0, 2),
    value => boxRuntimeInt(instance, value), () => { throw primary; }), error => {
      assert.deepEqual(error.errors, [primary, cleanup, cleanup]);
      assert.equal(error.cause, primary);
      return true;
    });
  assert.equal(live.size, 0);
  assert.throws(() => withRuntimeOwnedValues(instance, [wide[0]],
    value => boxRuntimeInt(instance, value), () => boxRuntimeInt(instance, wide[5]), true),
    error => error instanceof AggregateError && error.errors.length === 2);
  assert.equal(live.size, 0, 'unpublished result survived failed argument cleanup');
}
for (const missing of ['molt_int_from_i64', 'molt_exception_pending_fast', 'molt_dec_ref_obj']) {
  const {instance, events} = runtime();
  delete instance.exports[missing];
  assert.throws(() => boxRuntimeInt(instance, 1n), /missing ownership export/);
  assert.throws(() => makeRuntimeIntList(instance, []), /missing ownership export/);
  assert.equal(events.length, 0);
}
{
  const {instance, events} = runtime();
  instance.exports.molt_exception_pending_fast = () => 1n;
  assert.throws(() => boxRuntimeInt(instance, 1n), /before integer allocation/);
  assert.equal(events.length, 0);
}
""",
        tmp_path,
    )


@pytest.mark.parametrize(
    ("host", "binding", "next_binding"),
    [
        ("run_wasm.js", "makeHostArgObject", "runHostExportCalls"),
        ("browser_host.js", "makeBrowserHostArgObject", "parseIPv4"),
        ("browser_embed.js", "makeHostArg", "nativeCallableMap"),
    ],
)
def test_host_integer_materializers_preserve_exact_inputs(
    tmp_path: Path, host: str, binding: str, next_binding: str
) -> None:
    source = (ROOT / "wasm" / host).read_text(encoding="utf-8")
    materializer = source[
        source.index(f"const {binding} =") : source.index(f"const {next_binding} =")
    ]
    _run_node(
        "const materializer = "
        + json.dumps(materializer)
        + ";\nconst binding = "
        + json.dumps(binding)
        + ";\nconst lifecyclePath = "
        + json.dumps(str(ROOT / "wasm/runtime_lifecycle.js"))
        + ";\n"
        + r"""
const assert = require('node:assert/strict');
const vm = require('node:vm');
const {boxRuntimeInt} = require(lifecyclePath);
const seen = [];
const runtime = {exports: {
  molt_int_from_i64(value) { seen.push(value); return 1000n; },
  molt_exception_pending_fast() { return 0n; },
  molt_dec_ref_obj() {},
}};
const context = {boxRuntimeInt, runtimeInstance: runtime,
  makeListIntObject: values => values,
  makeListIntObjectWithRuntime: (_runtime, values) => values,
  makeRuntimeIntList: (_runtime, values) => values};
vm.createContext(context);
vm.runInContext(materializer + '\nthis.materialize = ' + binding + ';', context);
const materialize = value => binding === 'makeHostArgObject'
  ? context.materialize(value) : context.materialize(runtime, {}, value);
for (const value of ['9223372036854775807', '-9223372036854775808', 1n << 46n, 7]) {
  assert.equal(materialize({kind: 'int', value}), 1000n);
  assert.equal(seen.at(-1), BigInt(value));
}
const values = ['9223372036854775807', -(1n << 63n), 7];
assert.equal(materialize({kind: 'list_int', value: values}), values);
for (const value of [Number.MAX_SAFE_INTEGER + 1, 1.5, '9223372036854775808', null]) {
  const count = seen.length;
  assert.throws(() => materialize({kind: 'int', value}), /host integer/);
  assert.equal(seen.length, count);
}
if (binding !== 'makeHostArgObject') {
  materialize((1n << 63n) - 1n);
  assert.equal(seen.at(-1), (1n << 63n) - 1n);
  assert.throws(() => materialize(Number.MAX_SAFE_INTEGER + 1), /host integer/);
}
""",
        tmp_path,
    )


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
    runLinked: async () => {
      events.push('startup');
      if (failure) throw new Error(failure);
    },
    runDirectLink: async () => {
      events.push('startup');
      if (failure) throw new Error(failure);
    },
    runHostExportCalls: async () => events.push('host-export'),
    pendingRuntimeExceptionMessage(instance) { return instance === runtime ? failure : null; },
    wasiExitCode: null, maybeDumpRuntimeProfile() {},
    disposeRuntimeAndHost: async () => {
      events.push('dispose', 'shutdown');
      return [];
    },
    combinedError: errors => errors.length === 1 ? errors[0] : new AggregateError(errors),
  };
  vm.createContext(context);
  vm.runInContext(runMainSource + '\nthis.invoke = runMain;', context);
  if (failure) {
    await assert.rejects(context.invoke(), error => error.message === failure);
    assert.deepEqual(events, ['startup', 'dispose', 'shutdown']);
  } else {
    assert.equal(await context.invoke(), 0);
    assert.deepEqual(events, ['startup', 'host-export', 'dispose', 'shutdown']);
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


def test_node_finite_owner_keeps_hosts_live_through_runtime_shutdown(
    tmp_path: Path,
) -> None:
    source = (ROOT / "wasm/run_wasm.js").read_text(encoding="utf-8")
    start = source.index("let finiteDisposalPromise = null;")
    end = source.index("\nconst socketHostNew =", start)
    owner = source[start:end]
    _run_node(
        "const ownerSource = "
        + json.dumps(owner)
        + ";\n"
        + r"""
const vm = require('node:vm');
const assert = require('node:assert/strict');
const events = [];
const runtimeFailure = new Error('runtime shutdown');
const hostFailure = new Error('host close');
const context = {
  hostLifecycleState: 'live',
  runtimeInstance: {},
  runtimeLifetime: () => ({dispose() {
    assert.equal(context.hostLifecycleState, 'runtime-shutdown');
    events.push('runtime-shutdown', 'atexit-host-use');
    throw runtimeFailure;
  }}),
  shutdownHostWorkers: async () => {
    assert.equal(context.hostLifecycleState, 'host-closed');
    events.push('host-close');
    throw hostFailure;
  },
};
vm.createContext(context);
vm.runInContext(ownerSource + '\nthis.disposeOwner = disposeRuntimeAndHost;', context);
(async () => {
  const first = await context.disposeOwner();
  const second = await context.disposeOwner();
  assert.equal(first.length, 2);
  assert.equal(first[0], runtimeFailure);
  assert.equal(first[1], hostFailure);
  assert.equal(second, first);
  assert.deepEqual(events, ['runtime-shutdown', 'atexit-host-use', 'host-close']);
})().catch(error => { console.error(error); process.exitCode = 1; });
""",
        tmp_path,
    )


def test_node_host_cleanup_attempts_siblings_and_retains_live_process_custody(
    tmp_path: Path,
) -> None:
    source = (ROOT / "wasm/run_wasm.js").read_text(encoding="utf-8")
    start = source.index("const shutdownHostWorkers = async () => {")
    end = source.index("\nlet finiteDisposalPromise = null;", start)
    cleanup = source[start:end]
    _run_node(
        "const cleanupSource = "
        + json.dumps(cleanup)
        + ";\n"
        + r"""
const vm = require('node:vm');
const assert = require('node:assert/strict');
const events = [];
const socketFailure = new Error('socket close failed synchronously');
const dbFailure = new Error('db close rejected asynchronously');
const childFailure = new Error('child termination error');
const reapedFailure = new Error('error retained before reaping');
const socketWorkerClient = {close() { events.push('socket-close'); throw socketFailure; }};
const dbWorkerClient = {close() { events.push('db-close'); return Promise.reject(dbFailure); }};
const wsHandles = new Map([[1, {state: 'open', ws: {close() { events.push('ws-close'); }}}]]);
const processHandles = new Map();
const entry = {
  stdin: {destroyed: false, destroy() { events.push('stdin-destroy'); this.destroyed = true; }},
  stdout: {destroyed: false, off(name) { events.push(`stdout-off:${name}`); },
    destroy() { events.push('stdout-destroy'); this.destroyed = true; }},
  stderr: {destroyed: false, off(name) { events.push(`stderr-off:${name}`); },
    destroy() { events.push('stderr-destroy'); this.destroyed = true; }},
  exitObserved: false, exitCode: null, hostCleanupErrors: [],
  hostCleanupErrorsReported: 0, listeners: null, child: null,
};
entry.listeners = {
  onStdoutData() {}, onStdoutEnd() {}, onStderrData() {}, onStderrEnd() {},
  onExit() { entry.exitObserved = true; },
  onError(error) { entry.hostCleanupErrors.push(error); },
};
entry.child = {
  exitCode: null, signalCode: null,
  off(name) { events.push(`live-child-off:${name}`); },
  kill(signal) {
    events.push(`kill:${signal}`);
    entry.listeners.onError(childFailure);
    return false;
  },
};
processHandles.set(42, entry);
const reapedEntry = {
  child: {exitCode: 0, signalCode: null,
    off(name) { events.push(`reaped-child-off:${name}`); }},
  listeners: {onExit() {}, onError() {}},
  exitObserved: true, hostCleanupErrors: [reapedFailure], hostCleanupErrorsReported: 0,
};
processHandles.set(43, reapedEntry);
const context = {
  socketWorkerClient, dbWorkerClient, wsHandles, processHandles,
  workerClientRetirements: [{failed: true, error: null, promise: Promise.resolve()}],
  PROCESS_HOST_CLOSE_TIMEOUT_MS: 5, setTimeout, performance,
  combinedError: errors => new AggregateError(errors),
};
vm.createContext(context);
vm.runInContext(cleanupSource + '\nthis.cleanup = shutdownHostWorkers;', context);
(async () => {
  await assert.rejects(context.cleanup(), error => {
    assert.equal(error.errors.includes(socketFailure), true);
    assert.equal(error.errors.includes(dbFailure), true);
    assert.equal(error.errors.includes(childFailure), true);
    assert.equal(error.errors.includes(reapedFailure), true);
    assert.equal(error.errors.includes(null), true, 'falsy retirement failures must remain visible');
    assert.match(String(error.errors.find(item => /failed to force-stop/.test(String(item)))), /42/);
    assert.match(String(error.errors.find(item => /not reaped/.test(String(item)))), /42/);
    return true;
  });
  assert.equal(context.processHandles.has(42), true, 'live child custody must remain held');
  assert.equal(context.processHandles.has(43), false, 'reaped child custody must be released');
  assert.equal(events.includes('db-close'), true, 'sync sibling failure must not skip db close');
  assert.equal(events.includes('ws-close'), true, 'sync sibling failure must not skip websocket close');
  assert.equal(events.includes('kill:SIGKILL'), true);
  assert.equal(events.some(event => event.startsWith('live-child-off:')), false,
    'child error and exit listeners must remain until reaping is observed');
  assert.equal(events.includes('reaped-child-off:error'), true);
  assert.equal(events.includes('stdin-destroy'), true);
  assert.equal(events.includes('stdout-destroy'), true);
  assert.equal(events.includes('stderr-destroy'), true);
})().catch(error => { console.error(error); process.exitCode = 1; });
""",
        tmp_path,
    )


def test_node_worker_transport_close_preserves_all_failures(tmp_path: Path) -> None:
    source = (ROOT / "wasm/run_wasm.js").read_text(encoding="utf-8")
    start = source.index("const closeWorkerTransport =")
    end = source.index("\nlet workerClientRetirements =", start)
    close_transport = source[start:end]
    _run_node(
        "const closeSource = "
        + json.dumps(close_transport)
        + ";\n"
        + r"""
const vm = require('node:vm');
const assert = require('node:assert/strict');
const events = [];
const portFailure = new Error('port close failed');
const workerFailure = new Error('worker termination failed');
const context = {
  Promise,
  combinedError: errors => new AggregateError(errors),
};
vm.createContext(context);
vm.runInContext(closeSource + '\nthis.closeTransport = closeWorkerTransport;', context);
(async () => {
  await assert.rejects(
    context.closeTransport(
      {terminate() { events.push('terminate'); return Promise.reject(workerFailure); }},
      {close() { events.push('port-close'); throw portFailure; }},
    ),
    error => {
      assert.equal(error.errors.length, 2);
      assert.equal(error.errors[0], portFailure);
      assert.equal(error.errors[1], workerFailure);
      return true;
    },
  );
  assert.deepEqual(events, ['port-close', 'terminate']);
})().catch(error => { console.error(error); process.exitCode = 1; });
""",
        tmp_path,
    )


def test_node_db_completed_responses_precede_terminal_failure(tmp_path: Path) -> None:
    source = (ROOT / "wasm/run_wasm.js").read_text(encoding="utf-8")
    start = source.index("class DbWorkerClient")
    end = source.index("\nlet dbWorkerClient = null;", start)
    db_client = source[start:end]
    _run_node(
        "const clientSource = "
        + json.dumps(db_client)
        + ";\n"
        + r"""
const vm = require('node:vm');
const assert = require('node:assert/strict');
const messages = [
  {message: {type: 'error', message: 'port terminal'}},
  {message: {type: 'response', response: {requestId: 1, status: 'Ok'}}},
];
const context = {
  receiveMessageOnPort: () => messages.shift() || undefined,
  Buffer,
};
vm.createContext(context);
vm.runInContext(clientSource + '\nthis.DbWorkerClient = DbWorkerClient;', context);
const client = Object.create(context.DbWorkerClient.prototype);
client.port = {};
client.pendingErrors = ['worker exit'];
client.pending = new Map([[1, {streamHandle: 11n}], [2, {streamHandle: 22n}]]);
const events = [];
client._deliverResponse = (streamHandle) => events.push(`response:${streamHandle}`);
client._failAll = message => events.push(`terminal:${message}`);
client._drainResponses();
assert.deepEqual(events, [
  'response:11',
  'terminal:worker exit',
  'terminal:port terminal',
]);
assert.equal(client.pending.has(1), false);
assert.equal(client.pending.has(2), true);
""",
        tmp_path,
    )


@pytest.mark.parametrize(
    "mode", ["live-child", "truncated-eof", "invalid-frame", "oversized-frame"]
)
def test_node_db_shutdown_closes_real_child_and_retains_terminal_frames(
    tmp_path: Path, mode: str
) -> None:
    source = (ROOT / "wasm/run_wasm.js").read_text(encoding="utf-8")
    shutdown = source[
        source.index("const requestDbWorkerShutdown =") : source.index(
            "\nconst closeWorkerTransport ="
        )
    ]
    _run_node(
        "const workerPath = "
        + json.dumps(str(ROOT / "wasm/run_wasm.js"))
        + ";\nconst childPath = "
        + json.dumps(str(tmp_path / "db-child.cjs"))
        + ";\nconst mode = "
        + json.dumps(mode)
        + ";\nconst shutdownSource = "
        + json.dumps(shutdown)
        + ";\n"
        + r"""
const assert = require('node:assert/strict');
const fs = require('node:fs');
const net = require('node:net');
const vm = require('node:vm');
const {Worker, MessageChannel} = require('node:worker_threads');
const diagnostic = {'truncated-eof': 'truncated response frame',
  'invalid-frame': 'response decode failed', 'oversized-frame': 'frame too large'}[mode];
const fixture = `
  const net = require('node:net');
  const mode = ${JSON.stringify(mode)};
  // A bounded fixture lifetime prevents a failing test from orphaning a server.
  // The production shutdown acknowledgement must arrive well before this timer.
  setTimeout(() => process.exit(124), 10000).unref();
  process.stdin.once('data', () => {
    const respond = (port) => {
      const body = Buffer.from(JSON.stringify({request_id: 1, status: 'Ok',
        codec: 'raw', payload_b64: Buffer.from(String(port)).toString('base64')}));
      const header = Buffer.alloc(4); header.writeUInt32LE(body.length);
      let tail = Buffer.alloc(0);
      if (mode === 'truncated-eof') tail = Buffer.from([3, 0, 0, 0, 120]);
      if (mode === 'invalid-frame') tail = Buffer.from([1, 0, 0, 0, 120]);
      if (mode === 'oversized-frame') tail = Buffer.from([255, 255, 255, 255]);
      const frame = Buffer.concat([header, body, tail]);
      if (mode !== 'live-child') {
        process.stdin.destroy(); process.stdout.end(frame);
      } else process.stdout.write(frame);
    };
    if (mode === 'live-child') {
      const server = net.createServer();
      server.listen({host: '127.0.0.1', port: 0, exclusive: true}, () => respond(server.address().port));
    } else respond(0);
  });
`;
fs.writeFileSync(childPath, fixture);
const context = {MessageChannel, setTimeout, clearTimeout, DB_WORKER_CLOSE_TIMEOUT_MS: 250};
vm.createContext(context);
vm.runInContext(shutdownSource + '\nthis.shutdown = requestDbWorkerShutdown;', context);
const worker = new Worker(workerPath, {workerData: {
  kind: 'molt_db_host', cmd: [process.execPath, childPath],
}});
const channel = new MessageChannel();
const messages = [];
const workerErrors = [];
let workerExited = false;
worker.on('error', error => workerErrors.push(error));
worker.on('exit', () => { workerExited = true; });
channel.port1.on('message', message => messages.push(message));
const waitFor = async (predicate) => {
  const deadline = performance.now() + 3000;
  while (!predicate()) {
    assert.equal(workerErrors.length, 0, String(workerErrors[0]));
    assert.ok(performance.now() < deadline, `missing worker result: ${JSON.stringify(messages)}`);
    await new Promise(resolve => setTimeout(resolve, 5));
  }
};
(async () => {
  let shutdownStarted = false;
  try {
    worker.postMessage({type: 'init', port: channel.port2}, [channel.port2]);
    worker.postMessage({type: 'request', requestId: 1, entry: 'fixture', timeoutMs: 250, payload_b64: ''});
    await waitFor(() => messages.some(message => message.type === 'response'));
    const response = messages.find(message => message.type === 'response').response;
    assert.equal(response.requestId, 1);
    if (diagnostic) {
      await waitFor(() => messages.some(message => message.message?.includes(diagnostic)));
      assert.equal(messages[0].type, 'response', 'complete stdout frame must precede terminal diagnostics');
    }
    shutdownStarted = true;
    const started = performance.now();
    const {childClosed, errors} = await context.shutdown(worker);
    assert.equal(childClosed, true, 'only a closed child authorizes Worker termination');
    assert.ok(performance.now() - started < 2000, 'shutdown must not wait for fixture self-expiry');
    if (diagnostic) {
      assert.ok(errors.some(error => String(error).includes(diagnostic)),
        'cleanup must retain an already-closed child failure without a guest poll');
    } else {
      assert.equal(errors.length, 0, errors.map(String).join('; '));
      const port = Number(Buffer.from(response.payload).toString());
      assert.ok(port > 0);
      const probe = net.createServer();
      try {
        await new Promise((resolve, reject) => {
          probe.once('error', reject);
          probe.listen({host: '127.0.0.1', port, exclusive: true}, resolve);
        });
      } finally {
        if (probe.listening) await new Promise((resolve, reject) => probe.close(error => error ? reject(error) : resolve()));
      }
    }
    assert.equal(workerErrors.length, 0, String(workerErrors[0]));
    await waitFor(() => workerExited);
  } finally {
    if (!shutdownStarted) await context.shutdown(worker);
    channel.port1.close();
    // The Worker must drain its own ports after child closure; terminating it
    // here would hide a live Worker or destroy unproven child custody.
    await waitFor(() => workerExited);
  }
})().catch(error => { console.error(error); process.exitCode = 1; });
""",
        tmp_path,
    )


@pytest.mark.parametrize("close_error", [False, True])
def test_node_db_worker_drains_after_late_child_close(
    tmp_path: Path, close_error: bool
) -> None:
    source = (ROOT / "wasm/run_wasm.js").read_text(encoding="utf-8")
    worker = source[
        source.index("const dbHostWorkerMain =") : source.index("\nif (IS_DB_WORKER)")
    ]
    _run_node(
        "const workerSource = "
        + json.dumps(worker)
        + ";\nconst closeError = "
        + json.dumps(close_error)
        + ";\n"
        + r"""
const assert = require('node:assert/strict');
const vm = require('node:vm');
const {EventEmitter} = require('node:events');
(async () => {
  const events = [];
  const diagnostics = [];
  const timers = [];
  const child = new EventEmitter();
  Object.assign(child, {stdin: new EventEmitter(), stdout: new EventEmitter(),
    exitCode: null, signalCode: null, kill() { events.push('kill'); return true; }});
  const parentPort = new EventEmitter();
  parentPort.close = () => events.push('parent-close');
  const responsePort = {postMessage() {}, close() {
    events.push('response-close');
    if (closeError) throw new Error('response port close failed');
  }};
  const acknowledgements = [];
  const ackPort = {postMessage(message) { acknowledgements.push(message); },
    close() { events.push('ack-close'); }};
  const context = {parentPort, workerData: {cmd: ['fixture']},
    spawn: () => child, Buffer, process: {env: {}}, MAX_DB_FRAME_SIZE: 1024,
    DB_WORKER_CLOSE_TIMEOUT_MS: 250, writeFrame() {},
    console: {error: (...args) => diagnostics.push(args.map(String).join(' '))},
    setTimeout(callback, ms) { const timer = {callback, ms}; timers.push(timer); return timer; },
    clearTimeout(timer) { timer.cleared = true; },
  };
  vm.createContext(context);
  vm.runInContext(workerSource + '\ndbHostWorkerMain();', context);
  parentPort.emit('message', {type: 'init', port: responsePort});
  parentPort.emit('message', {type: 'request', requestId: 1});
  parentPort.emit('message', {type: 'shutdown', port: ackPort});
  assert.deepEqual(events, ['kill']);
  const deadline = timers.find(timer => timer.ms === 250);
  assert.ok(deadline);
  deadline.callback();
  await new Promise(setImmediate);
  assert.equal(acknowledgements.length, 1);
  assert.equal(acknowledgements[0].childClosed, false);
  assert.ok(acknowledgements[0].errors.some(error => error.includes('deadline')));
  assert.deepEqual(events, ['kill', 'ack-close']);
  // A terminal error arriving after the bounded acknowledgement still belongs
  // to this owner. Actual close, not the timeout, permits Worker port release.
  child.stdout.emit('error', new Error('late output failure'));
  child.emit('close', null, 'SIGKILL');
  await new Promise(setImmediate);
  assert.deepEqual(events, ['kill', 'ack-close', 'response-close', 'parent-close']);
  assert.ok(diagnostics.some(message => message.includes('late output failure')));
  if (closeError) assert.ok(diagnostics.some(message => message.includes('response port close failed')));
})().catch(error => { console.error(error); process.exitCode = 1; });
""",
        tmp_path,
    )


def test_node_finite_exit_records_status_without_abandoning_owners(
    tmp_path: Path,
) -> None:
    source = (ROOT / "wasm/run_wasm.js").read_text(encoding="utf-8")
    terminal = source[
        source.index("let terminationHandlersInstalled") : source.index(
            "\nmodule.exports ="
        )
    ]
    _run_node(
        "const terminalSource = "
        + json.dumps(terminal)
        + ";\n"
        + r"""
const assert = require('node:assert/strict');
const vm = require('node:vm');
const {EventEmitter} = require('node:events');
(async () => {
  for (const mode of ['success', 'nonzero', 'failure', 'wasi', 'signal']) {
    const process = new EventEmitter();
    process.exit = () => { throw new Error('forced exit abandons owned children'); };
    const module = {};
    const failure = new Error('guest failure');
    const diagnostics = [];
    let cleanups = 0;
    const context = {process, module, require: {main: module}, IS_DB_WORKER: false,
      IS_SOCKET_WORKER: false, traceRun: false, traceMark() {},
      console: {error: error => diagnostics.push(error)}, formatTraceError: String,
      isWasiExitSymbol: error => mode === 'wasi' && error === failure,
      wasiExitCode: 7, maybeDumpRuntimeProfile() {},
      combinedError: errors => errors[0],
      disposeRuntimeAndHost: async () => { cleanups++; return [failure]; },
      runMain: () => mode === 'signal' ? new Promise(() => {})
        : ['failure', 'wasi'].includes(mode) ? Promise.reject(failure)
        : Promise.resolve(mode === 'nonzero' ? 17 : 0),
    };
    vm.createContext(context);
    vm.runInContext(terminalSource, context);
    if (mode === 'signal') process.emit('SIGTERM');
    await new Promise(setImmediate);
    assert.equal(process.exitCode, {success: undefined, nonzero: 17, failure: 1, wasi: 7, signal: 143}[mode]);
    assert.equal(cleanups, mode === 'signal' ? 1 : 0);
    if (['failure', 'signal'].includes(mode)) assert.ok(diagnostics.includes(failure));
  }
})().catch(error => { console.error(error); process.exitCode = 1; });
""",
        tmp_path,
    )


def test_node_db_client_retains_owner_until_child_close_is_proven(
    tmp_path: Path,
) -> None:
    source = (ROOT / "wasm/run_wasm.js").read_text(encoding="utf-8")
    client = source[
        source.index("class DbWorkerClient") : source.index(
            "\nlet dbWorkerClient = null;"
        )
    ]
    _run_node(
        "const clientSource = "
        + json.dumps(client)
        + ";\n"
        + r"""
const assert = require('node:assert/strict');
const vm = require('node:vm');
(async () => {
  for (const childClosed of [false, true]) {
    const events = [];
    const failure = new Error('child close not proven');
    const context = {
      requestDbWorkerShutdown: async () => ({childClosed, errors: childClosed ? [] : [failure]}),
      closeWorkerTransport: async () => events.push('terminate'),
      combinedError: errors => errors.length === 1 ? errors[0] : new AggregateError(errors),
    };
    vm.createContext(context);
    vm.runInContext(clientSource + '\nthis.Client = DbWorkerClient;', context);
    const client = Object.create(context.Client.prototype);
    const worker = {};
    const port = {};
    Object.assign(client, {pending: new Map(), pendingErrors: [], worker, port, closePromise: null});
    const first = client.close();
    assert.equal(client.close(), first);
    if (childClosed) {
      await first;
      assert.deepEqual(events, ['terminate']);
      assert.equal(client.worker, null);
      assert.equal(client.port, null);
    } else {
      await assert.rejects(first, error => error === failure);
      assert.deepEqual(events, []);
      assert.equal(client.worker, worker);
      assert.equal(client.port, port);
    }
  }
})().catch(error => { console.error(error); process.exitCode = 1; });
""",
        tmp_path,
    )


@pytest.mark.parametrize("kind", ["db", "socket"])
@pytest.mark.parametrize("failure_kind", ["error", "null", "undefined"])
def test_node_dead_worker_client_is_closed_and_retained_before_replacement(
    tmp_path: Path, kind: str, failure_kind: str
) -> None:
    source = (ROOT / "wasm/run_wasm.js").read_text(encoding="utf-8")
    retirement = source[
        source.index("let workerClientRetirements =") : source.index(
            "\nclass DbWorkerClient"
        )
    ]
    if kind == "db":
        getter = source[
            source.index("let dbWorkerClient = null;") : source.index(
                "\nconst handleDbHost =", source.index("let dbWorkerClient = null;")
            )
        ].replace("let dbWorkerClient = null;", "let dbWorkerClient = oldClient;")
        invoke = "getDbWorkerClient"
        replacement = "DbWorkerClient"
        extra_context = "resolveWorkerCmd: () => ['worker'],"
    else:
        getter = source[
            source.index("let socketWorkerClient = null;") : source.index(
                "\nconst socketCall =", source.index("let socketWorkerClient = null;")
            )
        ].replace(
            "let socketWorkerClient = null;", "let socketWorkerClient = oldClient;"
        )
        invoke = "getSocketWorkerClient"
        replacement = "SocketWorkerClient"
        extra_context = ""
    _run_node(
        "const retirementSource = "
        + json.dumps(retirement)
        + ";\nconst getterSource = "
        + json.dumps(getter)
        + ";\nconst invokeName = "
        + json.dumps(invoke)
        + ";\nconst replacementName = "
        + json.dumps(replacement)
        + ";\nconst extraContextSource = "
        + json.dumps(extra_context)
        + ";\nconst failureKind = "
        + json.dumps(failure_kind)
        + ";\n"
        + r"""
const vm = require('node:vm');
const assert = require('node:assert/strict');
const events = [];
const retirementFailure = failureKind === 'error' ? new Error('retirement failed')
  : failureKind === 'null' ? null : undefined;
const oldClient = {
  dead: true,
  poll() { events.push('poll'); },
  close() { events.push('close-old'); return Promise.reject(retirementFailure); },
};
const Replacement = function () { events.push('create-new'); this.dead = false; };
const context = {
  oldClient, hostLifecycleState: 'live', Promise,
  [replacementName]: Replacement,
};
if (extraContextSource) Object.assign(context, Function(`return ({${extraContextSource}});`)());
vm.createContext(context);
vm.runInContext(
  retirementSource + getterSource +
    `\nthis.invoke = ${invokeName}; this.retirements = workerClientRetirements;`,
  context,
);
const replacementClient = context.invoke();
assert.equal(replacementClient.dead, false);
assert.deepEqual(events, invokeName === 'getDbWorkerClient'
  ? ['poll', 'close-old', 'create-new']
  : ['close-old', 'create-new']);
(async () => {
  assert.equal(context.retirements.length, 1);
  await context.retirements[0].promise;
  assert.equal(context.retirements[0].failed, true);
  assert.equal(context.retirements[0].error, retirementFailure);
})().catch(error => { console.error(error); process.exitCode = 1; });
""",
        tmp_path,
    )


def test_node_guest_initializers_are_lifetime_owned_bootstrap() -> None:
    source = (ROOT / "wasm/run_wasm.js").read_text(encoding="utf-8")
    direct = source[
        source.index("const runDirectLink =") : source.index("const runLinked =")
    ]
    linked = source[source.index("const runLinked =") : source.index("const runMain =")]
    assert (
        "runtimeLifetime(runtimeInst).initialize(() => {\n"
        "    initializeWasiContextForInstance" in direct
    )
    assert (
        "runtimeLifetime(linkedModule.instance).initialize(() => {\n"
        "    if (linkedMemory) initializeWasiForInstance" in linked
    )


def test_browser_gpu_dispatcher_terminates_once_and_rejects_resurrection(
    tmp_path: Path,
) -> None:
    source = (ROOT / "wasm/browser_gpu_dispatch.js").read_text(encoding="utf-8")
    start = source.index("const createWorkerWebGpuDispatcher =")
    end = source.index("\nexport const createBrowserGpuHost", start)
    dispatcher = source[start:end].replace(
        "new URL('./browser_gpu_worker.js', import.meta.url)",
        "'browser-gpu-worker'",
    )
    _run_node(
        "const dispatcherSource = "
        + json.dumps(dispatcher)
        + ";\n"
        + r"""
const vm = require('node:vm');
const assert = require('node:assert/strict');
const events = [];
const workers = [];
class FakeWorker {
  constructor() { this.id = workers.length + 1; this.listeners = {}; workers.push(this); }
  addEventListener(name, callback) { this.listeners[name] = callback; }
  postMessage() { events.push(`post:${this.id}`); }
  terminate() { events.push(`terminate:${this.id}`); }
}
const fakeAtomics = {
  wait() { return 'timed-out'; },
  store() {},
  notify() {},
};
const context = {
  Worker: FakeWorker,
  SharedArrayBuffer,
  Int32Array,
  Atomics: fakeAtomics,
  navigator: {gpu: {}},
};
vm.createContext(context);
vm.runInContext(dispatcherSource + '\nthis.create = createWorkerWebGpuDispatcher;', context);
const dispatcher = context.create({gpuKernelTimeoutMs: 1});
assert.throws(() => dispatcher.dispatchKernel({
  source: '', entry: '', grid: 1, workgroupSize: 1, bindings: [],
}), /timed out/);
const firstWorker = workers[0];
firstWorker.listeners.error({message: 'failed'});
assert.throws(() => dispatcher.dispatchKernel({
  source: '', entry: '', grid: 1, workgroupSize: 1, bindings: [],
}), /timed out/);
firstWorker.listeners.error({message: 'stale failed event'});
dispatcher.dispose();
dispatcher.dispose();
assert.deepEqual(events, ['post:1', 'terminate:1', 'post:2', 'terminate:2']);
assert.throws(() => dispatcher.dispatchKernel({
  source: '', entry: '', grid: 1, workgroupSize: 1, bindings: [],
}), /disposed/);
""",
        tmp_path,
    )


def test_browser_deferred_socket_messages_respect_resource_ownership(
    tmp_path: Path,
) -> None:
    source = (ROOT / "wasm/browser_host.js").read_text(encoding="utf-8")
    socket_callbacks = source[
        source.index("  const enqueueData =") : source.index("  const computeReady =")
    ]
    ws_callbacks = source[
        source.index("  const attachHandlers = (entry)") : source.index(
            "  const wsConnectHost ="
        )
    ]
    _run_node(
        "const socketCallbacks = "
        + json.dumps(socket_callbacks)
        + ";\nconst wsCallbacks = "
        + json.dumps(ws_callbacks)
        + ";\n"
        + r"""
const vm = require('node:vm');
const assert = require('node:assert/strict');
let completeBlob;
class DeferredBlob {
  arrayBuffer() { return new Promise(resolve => { completeBlob = resolve; }); }
}
async function check(mode) {
  const sockets = new Map();
  const entry = {handle: 7, queue: [], state: 'open', ws: {
    addEventListener() {},
  }};
  sockets.set(7, entry);
  const context = {disposed: false, sockets, Blob: DeferredBlob,
    ArrayBuffer, Uint8Array, ECONNRESET: 104, notifyWaiter() {},
    UTF8_ENCODER: new TextEncoder()};
  vm.createContext(context);
  vm.runInContext(socketCallbacks + wsCallbacks +
    '\nthis.attach = attachHandlers; this.enqueue = enqueueData; this.markError = markError;', context);
  context.attach(entry);
  entry.listeners.handleMessage({data: new DeferredBlob()});
  const core = {refCount: 1, recvQueue: [], state: 'open'};
  if (mode === 'dispose') context.disposed = true;
  if (mode === 'release') { sockets.delete(7); core.refCount = 0; }
  if (mode === 'peer-close') { entry.state = 'closed'; core.state = 'closed'; }
  completeBlob(new Uint8Array([42]).buffer);
  await Promise.resolve(); await Promise.resolve();
  context.enqueue(core, new Uint8Array([42]));
  context.markError(core, 104);
  const retained = mode === 'peer-close';
  assert.equal(entry.queue.length, retained ? 1 : 0);
  assert.equal(core.recvQueue.length, retained ? 1 : 0);
  entry.listeners.handleOpen();
  assert.equal(entry.state, retained ? 'closed' : 'open');
  // Remote EOF does not revoke ownership or discard already received bytes.
}
(async () => {
  for (const mode of ['dispose', 'release', 'peer-close']) await check(mode);
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
    start = source.index("const runtimeLifetimes =")
    delimiter = (
        "let activeReservedRuntimeCallables ="
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
        + ";\nconst lifetimePath = "
        + json.dumps(str(ROOT / "wasm/runtime_lifecycle.js"))
        + ";\n"
        + r"""
const vm = require('node:vm');
const assert = require('node:assert/strict');
const {runtimeExceptionPending} = require(bridgePath);
const {createRuntimeLifetime} = require(lifetimePath);
const names = {runtime_execution_enter: 'enter', runtime_execution_leave: 'leave', runtime_shutdown: 'shutdown'};
for (const status of [undefined, 0, () => 0, ignored => 0n, () => 2n, () => 0n, () => 1n]) {
  const events = [];
  const runtime = {exports: {
    enter() { events.push('enter'); return 1n; },
    leave() { events.push('leave'); },
    shutdown() { events.push('shutdown'); return 1n; },
    ...(status === undefined ? {} : {molt_exception_pending: status}),
  }};
  const context = {runtimeExceptionPending, createRuntimeLifetime, runtimeImportExportNames: names,
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
    molt_runtime_execution_enter() { events.push('enter'); return 1n; },
    molt_runtime_execution_leave() { events.push('leave'); },
    molt_runtime_shutdown() { events.push('shutdown'); return 1n; },
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
  assert.deepEqual(events, status === 'missing'
    ? ['enter', 'leave', 'shutdown'] : ['enter', 'main', 'leave', 'shutdown']);
  assert.equal(pending, status === 'pending' ? 1n : 0n,
               'host must not clear pending diagnostic state');
}
(async () => {
  for (const status of ['missing', 'pending', 'ok']) await exercise(status);
})().catch(error => { console.error(error); process.exitCode = 1; });
""",
        tmp_path,
    )


def test_shared_runtime_lifetime_custody_and_failure_ordering(tmp_path: Path) -> None:
    _run_node(
        "const {createRuntimeLifetime, combinedError} = require("
        + json.dumps(str(ROOT / "wasm/runtime_lifecycle.js"))
        + ");\n"
        + r"""
const assert = require('node:assert/strict');
const names = {runtime_execution_enter: 'enter', runtime_execution_leave: 'leave', runtime_shutdown: 'shutdown'};
function fixture({pending, leaveFailure, shutdownFailure, shutdownStatus = 1n} = {}) {
  const events = [];
  const exports = {
    enter() { events.push('enter'); return 17n; },
    leave(token) {
      assert.equal(token, 17n); events.push('leave');
      if (leaveFailure) throw leaveFailure;
    },
    shutdown() {
      events.push('shutdown');
      if (shutdownFailure) throw shutdownFailure;
      return shutdownStatus;
    },
  };
  const lifetime = createRuntimeLifetime({exports}, names, () => {
    if (pending) throw pending;
  });
  return {lifetime, events, exports};
}
function failedEntryFixture(kind) {
  const events = [];
  let pending = null;
  const entryFailure = new Error(`entry ${kind}`);
  const pendingFailure = new Error(`pending after ${kind}`);
  const exports = {
    enter() {
      events.push('enter'); pending = pendingFailure;
      if (kind === 'trap') throw entryFailure;
      return 0n;
    },
    leave() { events.push('leave'); },
    shutdown() { events.push('shutdown'); return 0n; },
  };
  const lifetime = createRuntimeLifetime({exports}, names, () => {
    if (pending) throw pending;
  });
  return {lifetime, events, entryFailure, pendingFailure};
}
{
  const {lifetime, events} = fixture();
  lifetime.initialize(() => events.push('initialize'));
  lifetime.execute(() => events.push('main'));
  lifetime.execute(() => events.push('callback'));
  lifetime.dispose(); lifetime.dispose();
  assert.deepEqual(events, ['initialize', 'enter', 'main', 'leave', 'enter', 'callback', 'leave', 'shutdown']);
  assert.throws(() => lifetime.execute(() => {}), /disposed/);
}
{
  const {lifetime, events} = fixture();
  lifetime.execute(() => assert.throws(() => lifetime.dispose(), /active execution/));
  lifetime.dispose();
  assert.deepEqual(events, ['enter', 'leave', 'shutdown']);
}
for (const primary of [new Error('main'), null, undefined]) {
  const cleanup = new Error('shutdown');
  const {lifetime, events} = fixture({shutdownFailure: cleanup});
  const errors = [];
  try { lifetime.execute(() => { throw primary; }); } catch (error) { errors.push(error); }
  try { lifetime.dispose(); } catch (error) { errors.push(error); }
  const aggregate = combinedError(errors);
  assert.equal(aggregate.errors[0], primary);
  assert.equal(aggregate.errors[1], cleanup);
  assert.equal(aggregate.cause, primary);
  assert.deepEqual(events, ['enter', 'leave', 'shutdown']);
  assert.throws(() => lifetime.dispose(), error => error === cleanup);
  assert.equal(events.filter(event => event === 'shutdown').length, 1);
}
{
  const primary = new Error('main'); const cleanup = new Error('leave');
  const {lifetime} = fixture({leaveFailure: cleanup});
  assert.throws(() => lifetime.execute(() => { throw primary; }),
    error => error.errors[0] === primary && error.errors[1] === cleanup);
  lifetime.dispose();
}
{
  const pending = new Error('startup pending');
  const {lifetime, events} = fixture({pending});
  assert.throws(() => lifetime.execute(() => events.push('unreachable')), error => error === pending);
  assert.throws(() => lifetime.dispose(), error => error === pending);
  assert.deepEqual(events, ['enter', 'leave', 'shutdown']);
}
{
  const {lifetime, exports, events} = fixture();
  delete exports.shutdown;
  assert.throws(() => lifetime.execute(() => {}), /runtime_shutdown/);
  lifetime.dispose();
  assert.deepEqual(events, [], 'failed admission must not repeat or enter guest cleanup');
}
{
  const {lifetime, events} = fixture({shutdownStatus: 0n});
  lifetime.execute(() => {});
  assert.throws(() => lifetime.dispose(), /shutdown did not complete/);
  assert.deepEqual(events, ['enter', 'leave', 'shutdown']);
}
{
  const bootstrapFailure = new Error('bootstrap failed');
  const {lifetime, events} = fixture({shutdownStatus: 0n});
  assert.throws(
    () => lifetime.initialize(() => { events.push('initialize'); throw bootstrapFailure; }),
    error => error === bootstrapFailure,
  );
  lifetime.dispose();
  assert.deepEqual(events, ['initialize', 'shutdown']);
}
{
  let pending = null;
  const setupFailure = new Error('setup pending');
  const exports = {
    enter() { return 1n; }, leave() {},
    shutdown() { return 0n; },
  };
  const lifetime = createRuntimeLifetime({exports}, names, () => {
    if (pending) throw pending;
  });
  assert.throws(
    () => lifetime.initialize(() => { pending = setupFailure; }),
    error => error === setupFailure,
  );
  assert.throws(() => lifetime.dispose(), error => error === setupFailure);
}
for (const kind of ['trap', 'zero']) {
  const {lifetime, events, entryFailure, pendingFailure} = failedEntryFixture(kind);
  assert.throws(
    () => lifetime.execute(() => events.push('unreachable')),
    kind === 'trap' ? error => error === entryFailure : /invalid execution-boundary token/,
  );
  assert.throws(() => lifetime.dispose(), error => error === pendingFailure);
  assert.deepEqual(events, ['enter', 'shutdown']);
}
{
  let queried = false;
  const {exports, events} = fixture({shutdownStatus: 0n});
  const lifetime = createRuntimeLifetime({exports}, names, () => { queried = true; });
  lifetime.admit(); lifetime.dispose(); lifetime.dispose();
  assert.equal(queried, true, 'admitted disposal must snapshot pure pending status');
  assert.deepEqual(events, ['shutdown']);
}
""",
        tmp_path,
    )


def test_isolate_import_trace_observes_numeric_id_without_guest_calls(
    tmp_path: Path,
) -> None:
    source = (ROOT / "wasm/run_wasm.js").read_text(encoding="utf-8")
    start = source.index("const traceIsolateImportCall =")
    end = source.index("\n};", start) + 3
    _run_node(
        "const traceSource = "
        + json.dumps(source[start:end])
        + ";\n"
        + r"""
const vm = require('node:vm');
const assert = require('node:assert/strict');
const lines = [];
const context = {traceIsolateImport: true, console: {error: line => lines.push(line)},
  readRuntimeStringBits() { throw new Error('trace entered guest'); }};
vm.createContext(context);
vm.runInContext(traceSource + '\nthis.trace = traceIsolateImportCall;', context);
context.trace('enter', [1n]); context.trace('exit', [1n], 0n);
assert.equal(lines.length, 2);
assert.match(lines[0], /moduleId=1/);
assert.match(lines[1], /result=0/);
""",
        tmp_path,
    )


def test_shared_runtime_disposer_retains_all_failures_and_active_custody(
    tmp_path: Path,
) -> None:
    _run_node(
        "const {createRuntimeLifetime, createRuntimeDisposer} = require("
        + json.dumps(str(ROOT / "wasm/runtime_lifecycle.js"))
        + ");\n"
        + r"""
const assert = require('node:assert/strict');
const assertThrown = (operation, expected) => {
  let caught = false;
  try { operation(); } catch (error) { caught = true; assert.equal(error, expected); }
  assert.equal(caught, true, 'expected a thrown value');
};
for (const failure of [null, undefined, false, 0, '', new Error('close failed')]) {
  const events = [];
  const dispose = createRuntimeDisposer(
    () => ({dispose() { events.push('shutdown'); }}),
    [() => { events.push('first'); throw failure; }, () => events.push('second')],
  );
  assertThrown(dispose, failure);
  assertThrown(dispose, failure);
  assert.deepEqual(events, ['shutdown', 'first', 'second']);
}
{
  const primary = new Error('runtime failure');
  const last = new Error('last host failure');
  const dispose = createRuntimeDisposer(
    () => ({dispose() { throw primary; }}), [() => { throw null; }, () => { throw last; }],
  );
  let aggregate;
  try { dispose(); } catch (error) { aggregate = error; }
  assert.equal(aggregate.cause, primary);
  assert.deepEqual(aggregate.errors, [primary, null, last]);
  assertThrown(dispose, aggregate);
}
{
  const events = [];
  const lifetime = createRuntimeLifetime({exports: {
    enter: () => 1n, leave() {}, shutdown() { events.push('shutdown'); return 1n; },
  }}, {runtime_execution_enter: 'enter', runtime_execution_leave: 'leave', runtime_shutdown: 'shutdown'}, () => {});
  const dispose = createRuntimeDisposer(() => lifetime, [() => events.push('host-close')]);
  lifetime.execute(() => {
    assert.throws(dispose, error => error.code === 'MOLT_RUNTIME_ACTIVE');
    assert.deepEqual(events, []);
  });
  dispose(); dispose();
  assert.deepEqual(events, ['shutdown', 'host-close']);
}
""",
        tmp_path,
    )


@pytest.mark.parametrize("family", ["socket", "websocket"])
def test_browser_socket_disposal_attempts_all_listeners_and_resources(
    tmp_path: Path, family: str
) -> None:
    source = (ROOT / "wasm/browser_host.js").read_text(encoding="utf-8")
    shared = source[
        source.index("const closeBrowserSocket =") : source.index(
            "let browserVfsModulePromise"
        )
    ]
    factory = (
        "export const createBrowserSocketHost ="
        if family == "socket"
        else "export const createBrowserWebSocketHost ="
    )
    start = source.index("  const dispose = () => {", source.index(factory))
    end = source.index("\n  return {", start)
    _run_node(
        "const sharedSource = "
        + json.dumps(shared)
        + ";\nconst disposalSource = "
        + json.dumps(source[start:end])
        + ";\n"
        + r"""
const assert = require('node:assert/strict');
const vm = require('node:vm');
const events = [];
const failures = [new Error('close failed'), new Error('open listener'), new Error('error listener')];
const entry = (name, fail) => ({state: 'open', listeners: {
  handleOpen() {}, handleMessage() {}, handleError() {}, handleClose() {},
}, ws: {
  close() { events.push(`${name}:close`); if (fail) throw failures[0]; },
  removeEventListener(event) {
    events.push(`${name}:remove:${event}`);
    if (fail && event === 'open') throw failures[1];
    if (fail && event === 'error') throw failures[2];
  },
}});
const first = entry('first', true);
const second = entry('second', false);
const context = {disposed: false, sockets: new Map([[1, first], [2, second]]),
  detached: new Map(), hostToSynthetic: new Map(), syntheticToHost: new Map(),
  notifyWaiter() {}, combinedError: errors => new AggregateError(errors)};
vm.createContext(context);
vm.runInContext(sharedSource + disposalSource + '\nthis.dispose = dispose;', context);
assert.throws(context.dispose, error => {
  assert.equal(error.errors.length, 1);
  const inner = error.errors[0].errors;
  assert.equal(inner.length, failures.length);
  failures.forEach((failure, index) => assert.equal(inner[index], failure));
  return true;
});
context.dispose();
assert.deepEqual(events, ['first:close', 'first:remove:open', 'first:remove:message',
  'first:remove:error', 'first:remove:close', 'second:close', 'second:remove:open',
  'second:remove:message', 'second:remove:error', 'second:remove:close']);
assert.equal(context.sockets.size, 0);
assert.equal(first.listeners, null);
assert.equal(second.listeners, null);
assert.equal(first.state, 'closed');
assert.equal(second.state, 'closed');
""",
        tmp_path,
    )


def test_browser_host_reusable_api_disposes_only_at_owner_end(tmp_path: Path) -> None:
    source = (ROOT / "wasm/browser_host.js").read_text(encoding="utf-8")
    boundary = source[
        source.index("const runtimeLifetimes =") : source.index(
            "let browserVfsModulePromise"
        )
    ]
    methods = source[
        source.index("  let hostExportsInitialized = false;") : source.index(
            "  try {\n    const overrides ="
        )
    ]
    start = source.index(
        "      return {\n        // Low-level inspection handle only.",
        source.index("export const loadMoltWasm"),
    )
    end = source.index("\n      };", start) + len("\n      };")
    owner = source[start:end].replace("return {", "this.host = {", 1)
    _run_node(
        "const boundary = "
        + json.dumps(boundary)
        + ";\n"
        + "const methods = "
        + json.dumps(methods)
        + ";\n"
        + "const owner = "
        + json.dumps(owner)
        + ";\n"
        + "const lifetimePath = "
        + json.dumps(str(ROOT / "wasm/runtime_lifecycle.js"))
        + ";\n"
        + r"""
const vm = require('node:vm');
const assert = require('node:assert/strict');
const {createRuntimeLifetime, createRuntimeDisposer, withRuntimeOwnedValues, combinedError} = require(lifetimePath);
const events = [];
const names = {runtime_execution_enter: 'enter', runtime_execution_leave: 'leave', runtime_shutdown: 'shutdown'};
const instance = {exports: {
  enter() { events.push('enter'); return 1n; },
  leave() { events.push('leave'); },
  shutdown() { events.push('shutdown'); return 1n; },
  molt_main() { events.push('main'); },
  molt_host_init() { events.push('host-init'); },
  sample() { events.push('export'); return 42n; },
  molt_dec_ref_obj() {},
}};
const context = {
  instance, state: {runtimeInstance: instance, memory: {}}, runtimeImportAbi: {export_names: names},
  createRuntimeLifetime, createRuntimeDisposer, withRuntimeOwnedValues, combinedError, runtimeExceptionPending: () => false,
  pendingRuntimeExceptionMessage: () => null, memoryExport: {}, memory: {}, env: {}, linkedTable: {},
  makeBrowserHostArgObject: () => 0n, decRefMaybeWithRuntime() {},
  decodeOwnedExportResult: (_runtime, _memory, value) => value,
  flushOwnedStdio() { events.push('flush'); },
  dbHost: {dispose() { events.push('db-close'); }},
  socketHost: {dispose() { events.push('socket-close'); }},
  wsHost: {dispose() { events.push('ws-close'); }},
  gpuHost: {dispose() { events.push('gpu-close'); }},
};
vm.createContext(context);
vm.runInContext(boundary + methods + owner, context);
(async () => {
  context.host.run();
  assert.equal(await context.host.invokeExport('sample'), 42n);
  context.host.run();
  assert.equal(events.includes('shutdown'), false);
  context.host.dispose(); context.host.dispose();
  assert.throws(() => context.host.run(), /disposed/);
  await assert.rejects(context.host.invokeExport('sample'), /disposed/);
  assert.deepEqual(events, ['enter', 'main', 'flush', 'leave', 'enter', 'export', 'leave',
    'enter', 'main', 'flush', 'leave', 'shutdown', 'db-close', 'socket-close', 'ws-close',
    'gpu-close', 'flush']);
})().catch(error => { console.error(error); process.exitCode = 1; });
""",
        tmp_path,
    )


def test_generated_split_worker_metadata_uses_owned_integer_transaction(
    tmp_path: Path,
) -> None:
    from molt.cli import _generate_split_worker_js
    from molt.capability_manifest import CapabilityManifest

    source = _generate_split_worker_js(
        resolved_capability_policy=CapabilityManifest().resolve(),
        shared_memory_initial_pages=8,
        shared_table_initial=16,
        shared_table_base=None,
    )
    start = source.index("      const makeCallBindFallback =")
    end = source.index("      const runtimeFallback =", start)
    _run_node(
        "const fallbackSource = "
        + json.dumps(source[start:end])
        + ";\nconst lifecyclePath = "
        + json.dumps(str(ROOT / "wasm/runtime_lifecycle.js"))
        + ";\n"
        + r"""
const assert = require('node:assert/strict');
const vm = require('node:vm');
const {boxRuntimeInt, withRuntimeOwnedValues} = require(lifecyclePath);
for (const failure of [null, 'call', 'cleanup']) {
  const events = [], primary = new Error('call failed'), cleanup = new Error('cleanup failed');
  const runtimeInstance = {exports: {
    molt_int_from_i64(value) { events.push(`box:${value}`); return value + 100n; },
    molt_exception_pending_fast() { return 0n; },
    molt_dec_ref_obj(value) {
      events.push(`drop:${value}`);
      if (failure === 'cleanup' && value === 100n) throw cleanup;
    },
  }};
  const context = {runtimeInstance, boxRuntimeInt, withRuntimeOwnedValues,
    callargsNew(arity, zero) {
      assert.equal(arity, 102n); assert.equal(zero, 100n); return 500n;
    },
    callargsPushPos(builder, item) {
      assert.equal(builder, 500n); events.push(`arg:${item}`);
    },
    callBindIc(zero, method, builder) {
      assert.equal(zero, 100n); assert.equal(method, 700n); assert.equal(builder, 500n);
      if (failure === 'call') throw primary;
      return 900n;
    },
  };
  vm.createContext(context);
  vm.runInContext(fallbackSource + '\nthis.invoke = makeCallBindFallback(2);', context);
  if (failure) {
    assert.throws(() => context.invoke(700n, 11n, 12n), error =>
      error === (failure === 'call' ? primary : cleanup));
  } else {
    assert.equal(context.invoke(700n, 11n, 12n), 900n);
    runtimeInstance.exports.molt_dec_ref_obj(900n);
  }
  assert.deepEqual(events, ['box:2', 'box:0', 'arg:11', 'arg:12', 'drop:100', 'drop:102',
    ...(failure === 'call' ? [] : ['drop:900'])]);
}
""",
        tmp_path,
    )


def test_generated_split_worker_finishes_after_lease_and_preserves_errors(
    tmp_path: Path,
) -> None:
    from molt.cli import _generate_split_worker_js
    from molt.capability_manifest import CapabilityManifest

    source = _generate_split_worker_js(
        resolved_capability_policy=CapabilityManifest().resolve(),
        shared_memory_initial_pages=8,
        shared_table_initial=16,
        shared_table_base=None,
    )
    start = source.index("    let rtInstance = null;")
    end = source.index('    const output = stdoutChunks.join("");', start)
    transaction = source[start:end]
    _run_node(
        "const transaction = "
        + json.dumps(transaction)
        + ";\n"
        + "const lifetimePath = "
        + json.dumps(str(ROOT / "wasm/runtime_lifecycle.js"))
        + ";\n"
        + r"""
const vm = require('node:vm');
const assert = require('node:assert/strict');
const {createRuntimeLifetime, combinedError} = require(lifetimePath);
async function exercise(fail, primary, failShutdown, failInstantiate = false) {
  const events = [];
  const cleanup = new Error('shutdown failed');
  const runtime = {exports: {
    enter() { events.push('enter'); return 1n; },
    leave() { events.push('leave'); },
    shutdown() { events.push('shutdown'); if (failShutdown) throw cleanup; return 1n; },
    _initialize() { events.push('initialize'); },
  }};
  const app = {exports: {molt_main() { events.push('main'); if (fail) throw primary; }}};
  const runtimeModule = {};
  const context = {
    createRuntimeLifetime, combinedError, runtimeExceptionPending: () => false,
    runtimeImportExportNames: {runtime_execution_enter: 'enter', runtime_execution_leave: 'leave', runtime_shutdown: 'shutdown'},
    runtimeModule, appModule: {}, appInstance: null, wasi: {}, hostEnv: {}, sharedTable: {}, wasmMemory: {},
    runtimeCallableTable: {}, appCallableTable: {}, verifyCallableTableEntries() {}, buildRuntimeImports: () => ({}),
    assetBytes: async () => null, vfs: {clear: () => events.push('vfs-clear')},
    stdoutDecoder: {decode: () => ''}, stderrDecoder: {decode: () => ''}, stdoutChunks: [], stderrChunks: [],
    ProcExit: class ProcExit {},
    WebAssembly: {instantiate: async module => {
      if (module === runtimeModule) return runtime;
      if (failInstantiate) throw primary;
      return app;
    }},
  };
  vm.createContext(context);
  vm.runInContext('this.run = async () => {' + transaction + '};', context);
  let caught = false;
  try { await context.run(); } catch (error) {
    caught = true;
    if (failShutdown && (fail || failInstantiate)) {
      assert.equal(error.errors[0], primary); assert.equal(error.errors[1], cleanup);
    } else assert.equal(error, failShutdown ? cleanup : primary);
  }
  assert.equal(caught, fail || failShutdown || failInstantiate);
  assert.deepEqual(events, failInstantiate ? ['shutdown', 'vfs-clear']
    : ['initialize', 'enter', 'main', 'leave', 'shutdown', 'vfs-clear']);
}
(async () => {
  await exercise(false, undefined, false);
  for (const primary of [new Error('main failed'), null, undefined]) {
    await exercise(true, primary, true);
    await exercise(true, primary, false);
    await exercise(false, primary, true, true);
  }
})().catch(error => { console.error(error); process.exitCode = 1; });
""",
        tmp_path,
    )
