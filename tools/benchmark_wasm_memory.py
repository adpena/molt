#!/usr/bin/env python3
"""Profile release WASM host RSS and declared linear-memory residency."""

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
const artifact = process.argv[2];
const mode = process.argv[3];

const collect = () => {
  if (global.gc) global.gc();
  return process.memoryUsage().rss;
};

const started = performance.now();
let bytes = null;
let memory = null;
let module = null;
let memoryPages = null;
if (mode !== 'baseline') {
  bytes = fs.readFileSync(artifact);
  const imports = bridge.parseWasmImports(bytes);
  memoryPages = imports.memory ? imports.memory.min : null;
  if (mode === 'memory') {
    memory = new WebAssembly.Memory({ initial: memoryPages });
  } else if (mode === 'compile') {
    module = new WebAssembly.Module(bytes);
  } else if (mode !== 'read') {
    throw new Error(`unknown mode ${mode}`);
  }
}
const rssBytes = collect();
process.stdout.write(JSON.stringify({
  elapsed_ms: performance.now() - started,
  rss_bytes: rssBytes,
  artifact_bytes: bytes ? bytes.length : 0,
  memory_pages: memoryPages,
  linear_memory_bytes: memory ? memory.buffer.byteLength : 0,
  compiled: module !== null,
}));
"""


def sample(artifact: Path, mode: str) -> dict[str, int | float | bool | None]:
    completed = subprocess.run(
        [
            "node",
            "--expose-gc",
            "-e",
            NODE_PROBE,
            str(ROOT / "wasm" / "loader_bridge.js"),
            str(artifact),
            mode,
        ],
        check=True,
        capture_output=True,
        text=True,
        cwd=ROOT,
    )
    return json.loads(completed.stdout)


def median_field(runs: list[dict[str, object]], field: str) -> float:
    return statistics.median(float(run[field]) for run in runs)


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("artifact", type=Path)
    parser.add_argument("--samples", type=int, default=7)
    parser.add_argument("--output", type=Path, required=True)
    args = parser.parse_args()
    if args.samples < 3:
        parser.error("A12 evidence requires at least 3 samples")
    artifact = args.artifact.resolve()
    if not artifact.is_file():
        parser.error(f"release artifact not found: {artifact}")

    modes = ("baseline", "memory", "read", "compile")
    runs = {mode: [] for mode in modes}
    for _ in range(args.samples):
        for mode in modes:
            runs[mode].append(sample(artifact, mode))

    medians = {
        mode: {
            "rss_bytes": median_field(mode_runs, "rss_bytes"),
            "elapsed_ms": median_field(mode_runs, "elapsed_ms"),
        }
        for mode, mode_runs in runs.items()
    }
    memory_pages = runs["memory"][0]["memory_pages"]
    linear_memory_bytes = runs["memory"][0]["linear_memory_bytes"]
    payload = {
        "schema_version": 1,
        "claim": "OPT-MATRIX-R7",
        "scenario": "real release WASM memory phase profile",
        "profile": "release",
        "hot_path": "V8 process startup, exact declared linear memory, artifact residency, and synchronous module compilation",
        "complexity": {
            "baseline": "O(1)",
            "memory": "O(declared initial pages)",
            "read": "O(artifact bytes)",
            "compile": "O(code and data sections)",
        },
        "artifact": str(artifact),
        "artifact_sha256": hashlib.sha256(artifact.read_bytes()).hexdigest(),
        "artifact_bytes": artifact.stat().st_size,
        "git_revision": subprocess.run(
            ["git", "rev-parse", "HEAD"],
            check=True,
            capture_output=True,
            text=True,
            cwd=ROOT,
        ).stdout.strip(),
        "samples": args.samples,
        "runs": runs,
        "medians": medians,
        "contract": {
            "memory_pages": memory_pages,
            "linear_memory_bytes": linear_memory_bytes,
            "page_bytes": 65536,
        },
        "attribution": {
            "artifact_metadata_rss_delta_bytes": medians["read"]["rss_bytes"]
            - medians["baseline"]["rss_bytes"],
            "declared_memory_committed_rss_delta_bytes": medians["memory"]["rss_bytes"]
            - medians["read"]["rss_bytes"],
            "compile_rss_delta_bytes": medians["compile"]["rss_bytes"]
            - medians["read"]["rss_bytes"],
        },
    }
    args.output.parent.mkdir(parents=True, exist_ok=True)
    args.output.write_text(json.dumps(payload, indent=2) + "\n", encoding="utf-8")
    print(json.dumps(payload, indent=2))
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
