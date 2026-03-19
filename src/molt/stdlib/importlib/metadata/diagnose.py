"""Diagnostic helpers for ``importlib.metadata`` package discovery."""

from __future__ import annotations

from _intrinsics import require_intrinsic as _require_intrinsic

import sys

from . import Distribution

_MOLT_CAPABILITIES_HAS = _require_intrinsic("molt_capabilities_has")


def inspect(path: str) -> None:
    print("Inspecting", path)
    dists = list(Distribution.discover(path=[path]))
    if not dists:
        return
    print("Found", len(dists), "packages:", end=" ")
    print(", ".join(dist.name for dist in dists))


def run() -> None:
    for path in sys.path:
        inspect(path)


if __name__ == "__main__":
    run()

globals().pop("_require_intrinsic", None)
