"""Execute optimizer policies against independently authored WASM."""

from __future__ import annotations

import json
import os
from pathlib import Path

import pytest

from molt import process_guard
from molt.node_runtime import admit_node_execution
from molt.wasm_optimization import wasm_link_policy
from molt.wasm_artifact import parse_wasm_imports
from molt.wasm_optimizer_identity import wasm_optimizer_invocation_identity
from tools.wasm_optimize import optimize
from tools import wasm_link_format, wasm_link_operations, wasm_link_pipeline
from tools.wasm_link_transaction import WasmArtifactState


ROOT = Path(__file__).resolve().parents[1]


@pytest.mark.parametrize(
    "apply_level", [True, False], ids=["linked", "split-reference"]
)
def test_optimizer_preserves_zero_segment_over_initialized_imported_memory(
    tmp_path: Path, apply_level: bool
) -> None:
    # (module (import "env" "memory" (memory 1))
    #   (func (export "get") (result i32) (i32.load8_u (i32.const 0)))
    #   (data (i32.const 0) "\00"))
    # No Molt emitter, linker, runtime build or optimizer generates this oracle.
    original = tmp_path / "original.wasm"
    original.write_bytes(
        bytes.fromhex(
            "0061736d01000000"
            "0105016000017f"
            "020f0103656e76066d656d6f7279020001"
            "03020100"
            "070701036765740000"
            "0a0901070041002d00000b"
            "0b07010041000b0100"
        )
    )
    optimized = tmp_path / "optimized.wasm"
    policy = wasm_link_policy("Oz")
    result = optimize(
        original,
        output_path=optimized,
        level=policy.level,
        extra_passes=policy.extra_passes,
        converge=policy.converge,
        apply_level=apply_level,
        preserve_debug=policy.preserve_debug,
        required_exports={"get"},
        optimizer_identity=wasm_optimizer_invocation_identity(),
        timeout=30,
    )
    assert result["ok"], result

    environment = dict(os.environ)
    node = admit_node_execution(source_root=ROOT, environment=environment)
    script = tmp_path / "observe.cjs"
    script.write_text(
        r"""
const assert = require('node:assert/strict');
const fs = require('node:fs');
for (const path of process.argv.slice(2)) {
  const bytes = fs.readFileSync(path);
  assert.ok(WebAssembly.validate(bytes), path);
  const module = new WebAssembly.Module(bytes);
  for (const prefill of [0, 0xa5]) {
    const memory = new WebAssembly.Memory({initial: 1});
    const view = new Uint8Array(memory.buffer);
    view.fill(prefill);
    const instance = new WebAssembly.Instance(module, {env: {memory}});
    assert.equal(instance.exports.get(), 0, `${path}: getter after ${prefill}`);
    assert.equal(view[0], 0, `${path}: active zero segment must be applied`);
    assert.equal(view[1], prefill, `${path}: unrelated memory must survive`);
  }
}
""",
        encoding="utf-8",
    )
    node.verify()
    completed = process_guard.run_completed_command(
        [str(node.runtime.path), str(script), str(original), str(optimized)],
        cwd=ROOT,
        env=environment,
        capture_output=True,
        timeout=30,
        memory_guard_prefix="MOLT_CROSS",
    )
    node.verify()
    assert completed.returncode == 0, json.dumps(
        {"stdout": completed.stdout, "stderr": completed.stderr}
    )


@pytest.mark.parametrize(
    "freestanding,level,wasi,converge",
    [
        (False, "Oz", True, False),
        (True, "Oz", True, True),
        (True, "O1", True, False),
        (True, None, True, False),
        (True, "Oz", False, False),
    ],
)
def test_post_link_optimizer_sees_freestanding_traps(
    tmp_path: Path, freestanding: bool, level: str | None, wasi: bool, converge: bool
) -> None:
    # Reuse the fixture scanner; Node independently validates and executes bytes.
    from tests.test_wasm_link_validation import _facts_provider

    # Independently authored module:
    #   import fd_sync(i32)->i32, env.mark()->i32, env.after()->i32
    #   run()->i32: drop(fd_sync(mark())); return after()
    #   get()->i32: extract lane 0 of v128.const(i32x4 7 0 0 0)
    # Stubbing must preserve mark(), the trap and get(), but an optimized
    # freestanding module must no longer need the host's after() implementation.
    name = wasm_link_format._write_string
    initial = wasm_link_operations.build_sections(
        [
            (1, b"\x02\x60\x01\x7f\x01\x7f\x60\x00\x01\x7f"),
            (
                2,
                b"\x03"
                + name("wasi_snapshot_preview1" if wasi else "env")
                + name("fd_sync")
                + b"\x00\x00"
                + name("env")
                + name("mark")
                + b"\x00\x01"
                + name("env")
                + name("after")
                + b"\x00\x01",
            ),
            (3, b"\x02\x01\x01"),
            (7, b"\x02" + name("run") + b"\x00\x03" + name("get") + b"\x00\x04"),
            (
                10,
                b"\x02\x09\x00\x10\x01\x10\x00\x1a\x10\x02\x0b"
                b"\x17\x00\xfd\x0c\x07\x00\x00\x00"
                + b"\x00" * 12
                + b"\xfd\x1b\x00\x0b",
            ),
        ]
    )
    path = tmp_path / "linked.wasm"
    artifact = WasmArtifactState.from_bytes(
        path, initial, facts_provider=_facts_provider
    )
    counts: dict[str, int | float] = {}
    attestation: dict[str, object] = {}
    optimize = level is not None
    assert wasm_link_pipeline._post_link_transform_stage(
        artifact,
        reference_data=None,
        preserve_exports={"run", "get"},
        preserve_debug=False,
        optimize=optimize,
        optimize_level=level or "Oz",
        split_runtime=False,
        freestanding=freestanding,
        optimizer_attestation=attestation,
        optimizer_identity=wasm_optimizer_invocation_identity() if optimize else None,
        operation_counts=counts,
        contract_app_exports=(),
        app_adapter_identity_map={},
        app_target_identity_map={},
    )
    imports = {
        (entry.module, entry.name) for entry in parse_wasm_imports(artifact.data)
    }
    traps = freestanding and wasi
    assert (("wasi_snapshot_preview1", "fd_sync") in imports) is (
        wasi and not freestanding
    )
    if traps and level == "Oz":
        assert ("env", "after") not in imports
    elif not traps or not optimize:
        assert ("env", "after") in imports
    assert counts.get("wasm_link_optimizer_requests", 0) == int(optimize)
    if optimize:
        assert attestation["optimization_converge"] is converge
        assert ("--converge" in attestation["pipeline"]) is converge
    else:
        assert attestation == {}
    artifact.persist()
    script = tmp_path / "observe.cjs"
    script.write_text(
        """
const assert = require('node:assert/strict');
const fs = require('node:fs');
const bytes = fs.readFileSync(process.argv[2]);
assert.ok(WebAssembly.validate(bytes));
let marked = 0, after = 0, synced = 0;
function fdSync(fd) {assert.equal(fd, 0); synced++; return 0}
const instance = new WebAssembly.Instance(new WebAssembly.Module(bytes), {
  env: {mark() {marked++; return 0}, after() {after++; return 43}, fd_sync: fdSync},
  wasi_snapshot_preview1: {fd_sync: fdSync}
});
assert.equal(instance.exports.get(), 7);
if (process.argv[3] === 'free') {
  assert.throws(() => instance.exports.run(), WebAssembly.RuntimeError);
  assert.equal(after, 0);
  assert.equal(synced, 0);
} else {
  assert.equal(instance.exports.run(), 43);
  assert.equal(after, 1);
  assert.equal(synced, 1);
}
assert.equal(marked, 1);
""",
        encoding="utf-8",
    )
    environment = dict(os.environ)
    node = admit_node_execution(
        source_root=Path(__file__).resolve().parents[1], environment=environment
    )
    node.verify()
    completed = process_guard.run_completed_command(
        [
            str(node.runtime.path),
            str(script),
            str(path),
            "free" if traps else "hosted",
        ],
        cwd=Path(__file__).resolve().parents[1],
        env=environment,
        capture_output=True,
        timeout=30,
        memory_guard_prefix="MOLT_WASM_TEST",
    )
    node.verify()
    assert completed.returncode == 0, completed.stderr
