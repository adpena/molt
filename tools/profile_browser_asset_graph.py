#!/usr/bin/env python3
from __future__ import annotations

import argparse
import json
import time
from pathlib import Path

from gen_browser_asset_graph import AssetSource, _read_manifest, scan_sources


ROOT = Path(__file__).resolve().parents[1]
DEFAULT_OUTPUT = ROOT / "tmp" / "browser_asset_graph" / "batch_profile.json"


def _profile(multiplier: int, sources: tuple[AssetSource, ...]) -> dict[str, object]:
    batch = tuple(
        AssetSource(
            f"{iteration}/{source.path}",
            source.role,
            source.source_type,
            source.authority,
            source.content,
        )
        for iteration in range(multiplier)
        for source in sources
    )
    started = time.perf_counter_ns()
    results, telemetry = scan_sources(batch)
    wall_ns = time.perf_counter_ns() - started
    return {
        "multiplier": multiplier,
        "node_elapsed_ms": telemetry["elapsed_ns"] / 1_000_000,
        "node_rss_bytes": telemetry["rss_bytes"],
        "process_count": 1,
        "result_count": len(results),
        "source_bytes": sum(len(source.content) for source in batch),
        "source_count": len(batch),
        "wall_ms": wall_ns / 1_000_000,
    }


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--output", type=Path, default=DEFAULT_OUTPUT)
    args = parser.parse_args()
    _groups, manifest_sources = _read_manifest()
    sources = tuple(
        source for source in manifest_sources.values() if source.source_type != "data"
    )
    cases = [_profile(multiplier, sources) for multiplier in (10, 100)]
    payload = {
        "cases": cases,
        "invariant": "one Node process and one source payload per batch",
        "schema_version": 1,
    }
    output = args.output.resolve()
    output.parent.mkdir(parents=True, exist_ok=True)
    output.write_text(
        json.dumps(payload, indent=2, sort_keys=True) + "\n", encoding="utf-8"
    )
    print(json.dumps(payload, sort_keys=True, separators=(",", ":")))
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
