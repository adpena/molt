#!/usr/bin/env python3
from __future__ import annotations

import argparse
import json
import re
import subprocess
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
CONTRACT = ROOT / "docs/spec/0016_VERIFIED_SUBSET_CONTRACT.md"
MANIFEST_RE = re.compile(r"```json\s*(\{.*?\})\s*```", re.DOTALL)


def load_manifest() -> dict:
    if not CONTRACT.exists():
        raise SystemExit(f"Missing contract file: {CONTRACT}")
    text = CONTRACT.read_text()
    match = MANIFEST_RE.search(text)
    if not match:
        raise SystemExit("Missing JSON manifest block in verified subset contract")
    return json.loads(match.group(1))


def validate_manifest(manifest: dict) -> int:
    errors: list[str] = []
    suites = manifest.get("differential_suites", [])
    if not suites:
        errors.append("manifest.differential_suites is empty")
    for path in suites:
        full_path = ROOT / path
        if not full_path.exists():
            errors.append(f"missing differential suite path: {path}")
    status_doc = manifest.get("status_doc")
    if status_doc:
        status_path = ROOT / status_doc
        if not status_path.exists():
            errors.append(f"missing status doc path: {status_doc}")
    if errors:
        print("Verified subset manifest errors:")
        for err in errors:
            print(f"  - {err}")
        return 1
    return 0


def run_differential_suites(manifest: dict) -> None:
    suites = manifest.get("differential_suites", [])
    for suite in suites:
        subprocess.check_call(
            [sys.executable, str(ROOT / "tests/molt_diff.py"), suite],
            cwd=ROOT,
        )


def main() -> int:
    parser = argparse.ArgumentParser(description="Verified subset contract checks")
    parser.add_argument(
        "command",
        nargs="?",
        default="check",
        choices={"check", "run"},
        help="check manifest integrity or run differential suites",
    )
    args = parser.parse_args()

    manifest = load_manifest()
    rc = validate_manifest(manifest)
    if rc != 0:
        return rc
    if args.command == "run":
        run_differential_suites(manifest)
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
