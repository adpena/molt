#!/usr/bin/env python3
from __future__ import annotations

import argparse
import json
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
SRC_DIR = ROOT / "src"
if str(SRC_DIR) not in sys.path:
    sys.path.insert(0, str(SRC_DIR))

from molt.debug.perf import build_perf_summary_payload, load_profile  # noqa: E402


def main() -> None:
    parser = argparse.ArgumentParser(description="Analyze molt_profile_json output from benchmark runs.")
    parser.add_argument("files", nargs="*", help="Profile JSON files or log files containing molt_profile_json lines.")
    args = parser.parse_args()

    profiles: dict[str, dict] = {}
    for file_arg in args.files:
        path = Path(file_arg)
        if not path.exists():
            print(f"Warning: file not found: {path}", file=sys.stderr)
            continue
        profile = load_profile(path)
        if profile is None:
            print(f"Warning: no profile data found in {path}", file=sys.stderr)
            continue
        profiles[path.stem] = profile

    if not profiles:
        print("No profile data provided. Use --help for usage.", file=sys.stderr)
        sys.exit(1)

    print(json.dumps(build_perf_summary_payload(profiles), indent=2, sort_keys=True))


if __name__ == "__main__":
    main()
