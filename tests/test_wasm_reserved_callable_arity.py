"""Reserved-runtime-callable arity reconciliation (loader_bridge host bridge).

Reserved runtime callables are invoked through the generic
``molt_call_indirectN`` fixed-arity lane, whose ``N`` is chosen by the caller's
positional-argument count rather than the callable's true C signature. A type
``__new__`` slot forwards ``(cls, *args)``, so a reserved callable whose declared
arity is smaller than the caller's indirect-call arity — e.g.
``molt_types_capsule_new(cls) -> u64`` invoked as ``capsule(cls, x, None, None)``
— must receive exactly its declared leading arguments and ignore the surplus,
mirroring the native C ABI. WASM cannot invoke a 1-param function with 4
operands, so the host bridge (`callReservedRuntimeCallable` in
``wasm/loader_bridge.js``) is the single reconciliation point. Under-supply stays
a hard error.

This locks the fix for the Pact witness ``molt_types_capsule_new`` arity-4
blocker.
"""

from __future__ import annotations

import shutil
import subprocess
from pathlib import Path

import pytest

ROOT = Path(__file__).resolve().parents[1]
LOADER_BRIDGE = ROOT / "wasm" / "loader_bridge.js"

# A self-contained Node harness that drives the real host bridge with a mock
# runtime-exports table, asserting the surplus-arg reconciliation, the
# exact-arity passthrough, and the under-supply hard error.
_NODE_HARNESS = r"""
const bridgePath = process.argv[process.argv.length - 1];
const { callReservedRuntimeCallable } = require(bridgePath);

function assert(cond, msg) {
  if (!cond) {
    throw new Error("ASSERT FAILED: " + msg);
  }
}

let received = null;
const runtimeExports = {
  // Declared arity 1: reads only `cls`, ignores any surplus operands.
  molt_types_capsule_new: (...args) => {
    received = args;
    return 123n;
  },
  // Declared arity 2: used to prove under-supply is still rejected.
  molt_needs_two: (a, b) => 456n,
  molt_cpython_abi_cext_call_trampoline: (...args) => {
    received = args;
    return 789n;
  },
};

// Case 1: declared arity 1, caller supplies 4 (the `(cls, *args)` __new__ lane).
const capsuleEntry = {
  runtimeExport: "molt_types_capsule_new",
  arity: 1,
  trampoline: false,
};
const result = callReservedRuntimeCallable({
  runtimeExports,
  memory: null,
  entry: capsuleEntry,
  indirectName: "molt_call_indirect4",
  args: [10n, 20n, 30n, 40n],
});
assert(received !== null, "reserved callable was not invoked");
assert(
  received.length === 1,
  "expected exactly 1 forwarded arg, got " + received.length + " [" + received + "]",
);
assert(received[0] === 10n, "expected cls=10n forwarded, got " + received[0]);
assert(result === 123n, "unexpected return value " + result);

// Case 2: exact arity still dispatches unchanged.
received = null;
callReservedRuntimeCallable({
  runtimeExports,
  memory: null,
  entry: capsuleEntry,
  indirectName: "molt_call_indirect1",
  args: [99n],
});
assert(
  received.length === 1 && received[0] === 99n,
  "exact-arity dispatch broken: [" + received + "]",
);

// Case 3: under-supply (fewer operands than declared arity) must still throw.
let threw = false;
try {
  callReservedRuntimeCallable({
    runtimeExports,
    memory: null,
    entry: { runtimeExport: "molt_needs_two", arity: 2, trampoline: false },
    indirectName: "molt_call_indirect1",
    args: [1n],
  });
} catch (err) {
  threw = true;
  assert(
    /arity mismatch/.test(err.message),
    "under-supply threw wrong error: " + err.message,
  );
}
assert(threw, "under-supply did not throw");

// Case 4: the C-extension trampoline owns the call-frame ABI itself. Its
// NaN-boxed closure is the callable registry id and must be forwarded with the
// argv pointer and argc rather than rejected as a closureless adapter.
received = null;
const closureBits = 9221401712017801271n;
const callFrameResult = callReservedRuntimeCallable({
  runtimeExports,
  memory: null,
  entry: {
    runtimeExport: "molt_cpython_abi_cext_call_trampoline",
    arity: 3,
    trampoline: true,
    trampolineAbi: "call_frame",
  },
  indirectName: "molt_call_indirect3",
  args: [closureBits, 4096n, 2n],
});
assert(
  received.length === 3 &&
    received[0] === closureBits &&
    received[1] === 4096n &&
    received[2] === 2n,
  "call-frame trampoline did not preserve closure/argv/argc: [" + received + "]",
);
assert(callFrameResult === 789n, "unexpected call-frame result " + callFrameResult);

console.log("OK");
"""


@pytest.mark.skipif(shutil.which("node") is None, reason="node is required")
def test_reserved_callable_surplus_args_reconciled_to_declared_arity() -> None:
    node = shutil.which("node")
    assert node is not None
    assert LOADER_BRIDGE.is_file(), f"missing host bridge: {LOADER_BRIDGE}"
    proc = subprocess.run(
        [node, "-e", _NODE_HARNESS, "--", str(LOADER_BRIDGE)],
        cwd=ROOT,
        stdout=subprocess.PIPE,
        stderr=subprocess.STDOUT,
        text=True,
        encoding="utf-8",
        errors="replace",
        check=False,
    )
    assert proc.returncode == 0, f"node harness failed:\n{proc.stdout}"
    assert proc.stdout.strip().splitlines()[-1] == "OK", proc.stdout
