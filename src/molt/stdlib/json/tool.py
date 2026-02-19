"""Minimal `json.tool` compatibility surface."""

import argparse
import json
import sys

from _intrinsics import require_intrinsic as _require_intrinsic

_MOLT_JSON_PARSE_SCALAR = _require_intrinsic("molt_json_parse_scalar_obj", globals())

if sys.version_info < (3, 13):
    try:
        from pathlib import Path
    except ImportError:
        class Path:  # pragma: no cover - runtime-only fallback for missing pathlib
            pass

if sys.version_info >= (3, 14):
    try:
        import re
    except ImportError:
        re = sys


def main():
    parser = argparse.ArgumentParser(prog="python -m json.tool")
    parser.add_argument("infile", nargs="?")
    parser.add_argument("outfile", nargs="?")
    args = parser.parse_args()
    if args.infile:
        with open(args.infile, "r", encoding="utf-8") as fh:
            data = json.load(fh)
    else:
        data = json.load(sys.stdin)
    text = json.dumps(data, sort_keys=True, indent=4)
    if args.outfile:
        with open(args.outfile, "w", encoding="utf-8") as fh:
            fh.write(text + "\n")
    else:
        sys.stdout.write(text + "\n")


if sys.version_info >= (3, 14):

    def can_colorize():
        return False


    def get_theme():
        return {}
