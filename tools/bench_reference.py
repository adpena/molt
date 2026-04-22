#!/usr/bin/env python3
from __future__ import annotations

import argparse
import json
from pathlib import Path
from typing import Any


REPO_ROOT = Path(__file__).resolve().parents[1]
DEFAULT_MANIFEST = REPO_ROOT / "bench/results/reference_manifest.json"


def _load_manifest(path: Path) -> dict[str, Any]:
    payload = json.loads(path.read_text(encoding="utf-8"))
    if int(payload.get("schema_version", 0)) != 1:
        raise ValueError("reference manifest must declare schema_version = 1")
    return payload


def _validate_lane_paths(manifest: dict[str, Any]) -> None:
    for lane in manifest.get("lanes", []):
        lane_path = REPO_ROOT / lane["path"]
        if not lane_path.is_dir():
            raise FileNotFoundError(f"lane path does not exist: {lane_path}")


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description="Plan a reference-lane bench run.")
    parser.add_argument("--manifest", type=Path, default=DEFAULT_MANIFEST)
    parser.add_argument(
        "--lane", action="append", default=[], help="limit to one or more lanes"
    )
    parser.add_argument("--json", action="store_true", help="emit JSON summary")
    args = parser.parse_args(argv)

    manifest = _load_manifest(args.manifest)
    _validate_lane_paths(manifest)

    lanes = manifest["lanes"]
    if args.lane:
        wanted = set(args.lane)
        lanes = [lane for lane in lanes if lane["id"] in wanted]
        missing = sorted(wanted - {lane["id"] for lane in lanes})
        if missing:
            raise ValueError(f"unknown lane(s): {', '.join(missing)}")

    summary = {
        "status": "ok",
        "manifest": str(args.manifest),
        "models": [model["id"] for model in manifest["models"]],
        "lanes": [
            {
                "id": lane["id"],
                "model": lane["model"],
                "backend": lane["backend"],
                "path": lane["path"],
                "enabled": lane["enabled"],
            }
            for lane in lanes
        ],
    }
    if args.json:
        print(json.dumps(summary, indent=2, sort_keys=True))
    else:
        print(f"reference bench plan: {len(summary['lanes'])} lane(s)")
        for lane in summary["lanes"]:
            print(f"- {lane['id']} -> {lane['path']}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
