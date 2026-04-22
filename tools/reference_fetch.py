#!/usr/bin/env python3
from __future__ import annotations

import argparse
import json
from pathlib import Path
from typing import Any

import tomllib


REPO_ROOT = Path(__file__).resolve().parents[1]
DEFAULT_PINS = REPO_ROOT / "bench/friends/reference/pins.toml"
DEFAULT_OUTPUT = REPO_ROOT / "bench/results/reference_manifest.json"


def _load_pins(path: Path) -> dict[str, Any]:
    payload = tomllib.loads(path.read_text(encoding="utf-8"))
    if int(payload.get("schema_version", 0)) != 1:
        raise ValueError("pins.toml must declare schema_version = 1")

    workspace = payload.get("workspace")
    if not isinstance(workspace, dict):
        raise ValueError("pins.toml must include a [workspace] table")

    models = payload.get("model")
    lanes = payload.get("lane")
    if not isinstance(models, list) or not models:
        raise ValueError("pins.toml must include at least one [[model]] table")
    if not isinstance(lanes, list) or not lanes:
        raise ValueError("pins.toml must include at least one [[lane]] table")

    return payload


def _normalize_manifest(pins: dict[str, Any]) -> dict[str, Any]:
    workspace = pins["workspace"]
    workspace_root = (REPO_ROOT / str(workspace["root"])).resolve()
    if not workspace_root.is_dir():
        raise FileNotFoundError(f"workspace root does not exist: {workspace_root}")

    models = []
    seen_models: set[str] = set()
    for entry in pins["model"]:
        model_id = str(entry["id"]).strip()
        if not model_id:
            raise ValueError("model ids must be non-empty")
        if model_id in seen_models:
            raise ValueError(f"duplicate model id: {model_id}")
        seen_models.add(model_id)
        models.append(
            {
                "id": model_id,
                "family": str(entry["family"]).strip(),
                "display_name": str(entry["display_name"]).strip(),
                "status": str(entry["status"]).strip(),
            }
        )

    lanes = []
    seen_lanes: set[str] = set()
    for entry in pins["lane"]:
        lane_id = str(entry["id"]).strip()
        model_id = str(entry["model"]).strip()
        lane_rel = str(entry["path"]).strip()
        lane_path = (workspace_root / lane_rel).resolve()
        if not lane_id:
            raise ValueError("lane ids must be non-empty")
        if lane_id in seen_lanes:
            raise ValueError(f"duplicate lane id: {lane_id}")
        if model_id not in seen_models:
            raise ValueError(f"lane {lane_id} references unknown model {model_id!r}")
        if not lane_path.is_dir():
            raise FileNotFoundError(f"lane path does not exist: {lane_path}")
        seen_lanes.add(lane_id)
        lane_record = {
            "id": lane_id,
            "model": model_id,
            "backend": str(entry["backend"]).strip(),
            "path": str(Path("bench/friends/reference") / lane_rel),
            "enabled": bool(entry.get("enabled", True)),
        }
        notes = entry.get("notes")
        if notes is not None:
            lane_record["notes"] = str(notes)
        lanes.append(lane_record)

    return {
        "schema_version": 1,
        "workspace": {
            "name": str(workspace["name"]).strip(),
            "root": str(workspace["root"]).strip(),
        },
        "models": models,
        "lanes": lanes,
    }


def _write_manifest(path: Path, manifest: dict[str, Any]) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(
        json.dumps(manifest, indent=2, sort_keys=True) + "\n", encoding="utf-8"
    )


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description="Generate the reference manifest.")
    parser.add_argument("--pins", type=Path, default=DEFAULT_PINS)
    parser.add_argument("--output", type=Path, default=DEFAULT_OUTPUT)
    parser.add_argument("--json", action="store_true", help="emit JSON summary")
    args = parser.parse_args(argv)

    pins = _load_pins(args.pins)
    manifest = _normalize_manifest(pins)
    _write_manifest(args.output, manifest)

    summary = {
        "status": "ok",
        "pins": str(args.pins),
        "output": str(args.output),
        "models": [model["id"] for model in manifest["models"]],
        "lanes": [lane["id"] for lane in manifest["lanes"]],
    }
    if args.json:
        print(json.dumps(summary, indent=2, sort_keys=True))
    else:
        print(f"wrote {args.output} from {args.pins}")
        print(f"models: {', '.join(summary['models'])}")
        print(f"lanes: {', '.join(summary['lanes'])}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
