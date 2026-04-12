#!/usr/bin/env python3
from __future__ import annotations

import argparse
import json
from pathlib import Path
from typing import Any

import tomllib


REPO_ROOT = Path(__file__).resolve().parents[1]
DEFAULT_PINS = REPO_ROOT / "bench/friends/reference/pins.toml"
DEFAULT_MANIFEST = REPO_ROOT / "bench/results/reference_manifest.json"


def _load_pins(path: Path) -> dict[str, Any]:
    payload = tomllib.loads(path.read_text(encoding="utf-8"))
    if int(payload.get("schema_version", 0)) != 1:
        raise ValueError("pins.toml must declare schema_version = 1")
    return payload


def _load_manifest(path: Path) -> dict[str, Any]:
    payload = json.loads(path.read_text(encoding="utf-8"))
    if int(payload.get("schema_version", 0)) != 1:
        raise ValueError("reference manifest must declare schema_version = 1")
    return payload


def _normalize_from_pins(pins: dict[str, Any]) -> dict[str, Any]:
    workspace = pins["workspace"]
    workspace_root = (REPO_ROOT / str(workspace["root"])).resolve()
    models = []
    for entry in pins.get("model", []):
        models.append(
            {
                "id": str(entry["id"]).strip(),
                "family": str(entry["family"]).strip(),
                "display_name": str(entry["display_name"]).strip(),
                "status": str(entry["status"]).strip(),
            }
        )

    lanes = []
    for entry in pins.get("lane", []):
        lane_rel = str(entry["path"]).strip()
        lane_path = (workspace_root / lane_rel).resolve()
        if not lane_path.is_dir():
            raise FileNotFoundError(f"lane path does not exist: {lane_path}")
        lane = {
            "id": str(entry["id"]).strip(),
            "model": str(entry["model"]).strip(),
            "backend": str(entry["backend"]).strip(),
            "path": str(Path("bench/friends/reference") / lane_rel),
            "enabled": bool(entry.get("enabled", True)),
        }
        notes = entry.get("notes")
        if notes is not None:
            lane["notes"] = str(notes)
        lanes.append(lane)

    return {
        "schema_version": 1,
        "workspace": {
            "name": str(workspace["name"]).strip(),
            "root": str(workspace["root"]).strip(),
        },
        "models": models,
        "lanes": lanes,
    }


def _compare_lists(left: list[dict[str, Any]], right: list[dict[str, Any]], key: str) -> list[str]:
    left_ids = [item[key] for item in left]
    right_ids = [item[key] for item in right]
    if left_ids != right_ids:
        return [f"{key}s differ: {left_ids!r} != {right_ids!r}"]
    return []


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description="Compare the reference manifest with pins.")
    parser.add_argument("--pins", type=Path, default=DEFAULT_PINS)
    parser.add_argument("--manifest", type=Path, default=DEFAULT_MANIFEST)
    parser.add_argument("--json", action="store_true", help="emit JSON summary")
    args = parser.parse_args(argv)

    pins = _load_pins(args.pins)
    manifest = _load_manifest(args.manifest)
    expected = _normalize_from_pins(pins)

    problems: list[str] = []
    if manifest["workspace"] != expected["workspace"]:
        problems.append(
            f"workspace differs: {manifest['workspace']!r} != {expected['workspace']!r}"
        )
    problems.extend(_compare_lists(manifest["models"], expected["models"], "id"))
    problems.extend(_compare_lists(manifest["lanes"], expected["lanes"], "id"))
    if manifest["lanes"] != expected["lanes"]:
        problems.append("lane records differ")
    if manifest["models"] != expected["models"]:
        problems.append("model records differ")

    summary = {
        "status": "ok" if not problems else "mismatch",
        "pins": str(args.pins),
        "manifest": str(args.manifest),
        "model_ids": [model["id"] for model in manifest["models"]],
        "lane_ids": [lane["id"] for lane in manifest["lanes"]],
        "problems": problems,
    }
    if args.json:
        print(json.dumps(summary, indent=2, sort_keys=True))
    else:
        if problems:
            print("mismatch")
            for problem in problems:
                print(problem)
        else:
            print(f"manifest matches pins: {args.manifest}")
    return 0 if not problems else 1


if __name__ == "__main__":
    raise SystemExit(main())
