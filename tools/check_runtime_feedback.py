#!/usr/bin/env python3
from __future__ import annotations

import argparse
import json
import sys
from pathlib import Path

SRC_ROOT = Path(__file__).resolve().parents[1] / "src"
if str(SRC_ROOT) not in sys.path:
    sys.path.insert(0, str(SRC_ROOT))

from molt._runtime_profile_schema import validate_process_profile  # noqa: E402


def _fail(msg: str) -> int:
    print(f"runtime-feedback-check: FAIL: {msg}", file=sys.stderr)
    return 1


def _validate(path: Path) -> int:
    if not path.exists():
        return _fail(f"missing file: {path}")
    try:
        payload = json.loads(path.read_text(encoding="utf-8"))
    except Exception as exc:  # noqa: BLE001
        return _fail(f"invalid JSON: {exc}")

    if error := validate_process_profile(payload):
        return _fail(error)
    print(f"runtime-feedback-check: OK: {path}")
    return 0


def main() -> int:
    parser = argparse.ArgumentParser(
        description="Validate Molt runtime feedback JSON schema."
    )
    parser.add_argument("path", help="Path to molt_runtime_feedback.json artifact")
    args = parser.parse_args()
    return _validate(Path(args.path))


if __name__ == "__main__":
    raise SystemExit(main())
