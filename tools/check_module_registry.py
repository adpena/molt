#!/usr/bin/env python3
"""Module registry projection checker (import bedrock, design doc 69 section 3).

The importable authority lives in src/molt/cli/module_registry.py. The build
pipeline calls it directly to derive the per-build registry from the binary-image
closure plan. This CLI re-derives registry_digest from emitted
module_registry.json artifacts and fails closed on projection drift.

Usage:
    python tools/check_module_registry.py --check <module_registry.json> [...]
"""

from __future__ import annotations

import argparse
import json
import sys
from pathlib import Path

_REPO_ROOT = Path(__file__).resolve().parents[1]
_SRC = _REPO_ROOT / "src"
if str(_SRC) not in sys.path:
    sys.path.insert(0, str(_SRC))

from molt.cli.module_registry import check_registry_json_payload  # noqa: E402


def _check(paths: list[Path]) -> int:
    status = 0
    for path in paths:
        try:
            payload = json.loads(path.read_text(encoding="utf-8"))
        except (OSError, json.JSONDecodeError) as exc:
            print(f"{path}: unreadable registry projection: {exc}")
            status = 1
            continue
        problems = check_registry_json_payload(payload)
        if problems:
            status = 1
            for problem in problems:
                print(f"{path}: {problem}")
        else:
            print(f"{path}: OK ({payload.get('registry_digest')})")
    return status


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument(
        "--check",
        nargs="+",
        type=Path,
        metavar="MODULE_REGISTRY_JSON",
        required=True,
        help="verify emitted module_registry.json projections against the "
        "checked-in registry authority (digest + derived fields)",
    )
    args = parser.parse_args(argv)
    return _check(list(args.check))


if __name__ == "__main__":
    raise SystemExit(main())
