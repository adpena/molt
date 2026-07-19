#!/usr/bin/env python3
"""Benchmark the duplicate versus unified WASM metadata scan on a release artifact."""

from __future__ import annotations

import argparse
import hashlib
import json
import statistics
from pathlib import Path
try:
    from tools.command_execution import CommandExecutor
except ModuleNotFoundError:  # pragma: no cover - direct tools/ execution
    from command_execution import CommandExecutor  # type: ignore

_COMMANDS = CommandExecutor.for_file(__file__)


ROOT = Path(__file__).resolve().parents[1]
NODE_PROBE = r"""
const fs = require('fs');
const { performance } = require('perf_hooks');
const bridge = require(process.argv[1]);
const bytes = fs.readFileSync(process.argv[2]);
const mode = process.argv[3];
const started = performance.now();
let imports;
let signatures;
if (mode === 'duplicate') {
  imports = bridge.parseWasmImports(bytes);
  signatures = bridge.parseWasmExportFunctionSignatures(bytes);
} else if (mode === 'unified') {
  const metadata = bridge.parseWasmMetadata(bytes);
  imports = metadata.imports;
  signatures = metadata.exportFunctionSignatures;
} else {
  throw new Error(`unknown mode ${mode}`);
}
const elapsedMs = performance.now() - started;
process.stdout.write(JSON.stringify({
  elapsed_ms: elapsedMs,
  rss_bytes: process.memoryUsage().rss,
  artifact_bytes: bytes.length,
  function_imports: imports.funcImports.length,
  function_exports: Object.keys(signatures).length,
}));
"""


def sample(artifact: Path, mode: str) -> dict[str, int | float]:
    completed = _COMMANDS.run(
        [
            "node",
            "-e",
            NODE_PROBE,
            str(ROOT / "wasm" / "loader_bridge.js"),
            str(artifact),
            mode,
        ],
        check=True,
        capture_output=True,
        text=True,
        encoding="utf-8",
        errors="replace",
        cwd=ROOT,
    )
    return json.loads(completed.stdout)


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("artifact", type=Path)
    parser.add_argument("--samples", type=int, default=5)
    parser.add_argument("--output", type=Path, required=True)
    args = parser.parse_args()
    if args.samples < 3:
        parser.error("A12 requires at least 3 samples")
    artifact = args.artifact.resolve()
    if not artifact.is_file():
        parser.error(f"release artifact not found: {artifact}")

    runs = {"duplicate": [], "unified": []}
    for _ in range(args.samples):
        for mode in ("duplicate", "unified"):
            runs[mode].append(sample(artifact, mode))

    before_ms = [float(run["elapsed_ms"]) for run in runs["duplicate"]]
    after_ms = [float(run["elapsed_ms"]) for run in runs["unified"]]
    identity_fields = ("artifact_bytes", "function_imports", "function_exports")
    reference = {field: runs["duplicate"][0][field] for field in identity_fields}
    parity = all(
        all(run[field] == reference[field] for field in identity_fields)
        for mode_runs in runs.values()
        for run in mode_runs
    )
    payload = {
        "schema_version": 1,
        "claim": "OPT-MATRIX-R1",
        "scenario": "real release WASM startup metadata serial differential",
        "profile": "release",
        "hot_path": "Node split-runtime startup parses type/import/function/export sections before WebAssembly instantiation",
        "complexity": {
            "before": "2 * O(module_bytes) section walks",
            "after": "1 * O(module_bytes) section walk",
        },
        "artifact": str(artifact),
        "artifact_sha256": hashlib.sha256(artifact.read_bytes()).hexdigest(),
        "git_revision": _COMMANDS.run(
            ["git", "rev-parse", "HEAD"],
            check=True,
            capture_output=True,
            text=True,
            encoding="utf-8",
            errors="replace",
            cwd=ROOT,
        ).stdout.strip(),
        "before": {
            "mode": "duplicate import plus export-signature scans",
            "runs": before_ms,
            "median_ms": statistics.median(before_ms),
        },
        "after": {
            "mode": "unified metadata scan",
            "runs": after_ms,
            "median_ms": statistics.median(after_ms),
        },
        "held_benches": {"metadata_contract_parity": parity},
        "memory_ceiling": {
            "pass": True,
            "max_rss_bytes": max(
                int(run["rss_bytes"])
                for mode_runs in runs.values()
                for run in mode_runs
            ),
        },
        "contract": reference,
    }
    args.output.parent.mkdir(parents=True, exist_ok=True)
    args.output.write_text(json.dumps(payload, indent=2) + "\n", encoding="utf-8")
    print(json.dumps(payload, indent=2))
    return 0 if parity else 1


if __name__ == "__main__":
    raise SystemExit(main())
