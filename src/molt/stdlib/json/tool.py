"""Minimal `json.tool` compatibility surface."""

import argparse
import importlib.util as _importlib_util
import json
import sys

from _intrinsics import require_intrinsic as _require_intrinsic

_MOLT_JSON_PARSE_SCALAR = _require_intrinsic("molt_json_parse_scalar_obj")


def _load_module(name: str):
    if _importlib_util.find_spec(name) is None:
        return None
    try:
        return __import__(name, fromlist=["*"])
    except Exception:  # pragma: no cover - runtime-only fallback
        return None


if sys.version_info < (3, 13):
    _pathlib = _load_module("pathlib")
    _path_type = None if _pathlib is None else getattr(_pathlib, "Path", None)
    if _path_type is None:

        class Path:  # pragma: no cover - runtime-only fallback for missing pathlib
            pass

    else:
        Path = _path_type

if sys.version_info >= (3, 14):
    _re = _load_module("re")
    if _re is None:
        re = sys
    else:
        re = _re


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


globals().pop("_require_intrinsic", None)
