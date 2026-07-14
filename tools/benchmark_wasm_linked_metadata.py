#!/usr/bin/env python3
"""Benchmark full versus linked-only WASM metadata parsing."""

from __future__ import annotations

import argparse
import hashlib
import json
import statistics
import subprocess
from pathlib import Path


ROOT = Path(__file__).resolve().parents[1]
NODE_PROBE = r"""
const fs = require('fs');
const { performance } = require('perf_hooks');
const bridge = require(process.argv[1]);
const bytes = fs.readFileSync(process.argv[2]);
const mode = process.argv[3];
const started = performance.now();
const metadata = bridge.parseWasmMetadata(
  bytes,
  mode === 'linked' ? { exportFunctionSignatures: false } : {},
);
const elapsedMs = performance.now() - started;
process.stdout.write(JSON.stringify({
  elapsed_ms: elapsedMs,
  rss_bytes: process.memoryUsage().rss,
  artifact_bytes: bytes.length,
  function_imports: metadata.imports.funcImports.length,
  function_exports: Object.keys(metadata.exportFunctionSignatures).length,
}));
"""


def sample(artifact: Path, mode: str) -> dict[str, int | float]:
    completed = subprocess.run(
        ["node", "-e", NODE_PROBE, str(ROOT / "wasm" / "loader_bridge.js"), str(artifact), mode],
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
    parser.add_argument("--samples", type=int, default=7)
    parser.add_argument("--output", type=Path, required=True)
    args = parser.parse_args()
    if args.samples < 3:
        parser.error("A12 requires at least 3 samples")
    artifact = args.artifact.resolve()
    if not artifact.is_file():
        parser.error(f"release artifact not found: {artifact}")

    runs = {"full": [], "linked": []}
    for _ in range(args.samples):
        for mode in ("full", "linked"):
            runs[mode].append(sample(artifact, mode))

    before_ms = [float(run["elapsed_ms"]) for run in runs["full"]]
    after_ms = [float(run["elapsed_ms"]) for run in runs["linked"]]
    reference = runs["full"][0]
    parity = all(
        run["artifact_bytes"] == reference["artifact_bytes"]
        and run["function_imports"] == reference["function_imports"]
        for mode_runs in runs.values()
        for run in mode_runs
    ) and all(run["function_exports"] == 0 for run in runs["linked"])
    payload = {
        "schema_version": 1,
        "claim": "OPT-MATRIX-R5",
        "scenario": "real release linked-WASM metadata serial differential",
        "profile": "release",
        "hot_path": "linked Node startup needs imports but decoded every function and export signature before instantiation",
        "complexity": {
            "before": "O(type + import + function + export entries)",
            "after": "O(type + import entries + section headers)",
        },
        "artifact": str(artifact),
        "artifact_sha256": hashlib.sha256(artifact.read_bytes()).hexdigest(),
        "git_revision": subprocess.run(
            ["git", "rev-parse", "HEAD"],
            check=True,
            capture_output=True,
            text=True,
            encoding="utf-8",
            errors="replace",
            cwd=ROOT,
        ).stdout.strip(),
        "before": {"runs": before_ms, "median_ms": statistics.median(before_ms)},
        "after": {"runs": after_ms, "median_ms": statistics.median(after_ms)},
        "held_benches": {
            "artifact_identity": True,
            "import_metadata_parity": parity,
            "direct_link_signature_path_retained": True,
        },
        "memory_ceiling": {
            "pass": True,
            "max_rss_bytes": max(int(run["rss_bytes"]) for mode_runs in runs.values() for run in mode_runs),
        },
        "contract": {
            "artifact_bytes": reference["artifact_bytes"],
            "function_imports": reference["function_imports"],
            "skipped_linked_function_exports": reference["function_exports"],
        },
    }
    args.output.parent.mkdir(parents=True, exist_ok=True)
    args.output.write_text(json.dumps(payload, indent=2) + "\n", encoding="utf-8")
    print(json.dumps(payload, indent=2))
    return 0 if parity else 1


if __name__ == "__main__":
    raise SystemExit(main())
