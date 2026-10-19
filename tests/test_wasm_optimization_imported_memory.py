"""Execute optimizer policies against independently authored imported-memory WASM."""

from __future__ import annotations

import json
import os
from pathlib import Path

import pytest

from molt import process_guard
from molt.node_runtime import admit_node_execution
from molt.wasm_optimization import wasm_link_policy
from molt.wasm_optimizer_identity import wasm_optimizer_invocation_identity
from tools.wasm_optimize import optimize


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
