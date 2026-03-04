from __future__ import annotations

import argparse
import json
import subprocess
import sys


def build_parser() -> argparse.ArgumentParser:
    parser = argparse.ArgumentParser(description="Fast code search wrapper for Molt")
    parser.add_argument("pattern", help="ripgrep pattern")
    parser.add_argument(
        "paths",
        nargs="*",
        default=["."],
        help="Paths to search (default: repo root)",
    )
    parser.add_argument(
        "--glob", action="append", default=[], help="Additional rg globs"
    )
    parser.add_argument("--ignore-case", action="store_true")
    parser.add_argument("--json", action="store_true", help="Emit JSON lines from rg")
    return parser


def main(argv: list[str] | None = None) -> int:
    args = build_parser().parse_args(argv)
    cmd = ["rg", "-n", "--hidden", "--no-heading"]
    if args.ignore_case:
        cmd.append("-i")
    for glob in args.glob:
        cmd.extend(["--glob", glob])
    if args.json:
        cmd.append("--json")
    cmd.append(args.pattern)
    cmd.extend(args.paths)

    proc = subprocess.run(cmd, check=False, capture_output=True, text=True)
    if args.json:
        rows = []
        for line in proc.stdout.splitlines():
            try:
                rows.append(json.loads(line))
            except json.JSONDecodeError:
                continue
        print(json.dumps(rows, indent=2))
    else:
        sys.stdout.write(proc.stdout)
    if proc.stderr:
        sys.stderr.write(proc.stderr)
    return int(proc.returncode)


if __name__ == "__main__":
    raise SystemExit(main())
