"""Diagnostic helpers for ``importlib.metadata`` package discovery."""

from __future__ import annotations


import sys

from . import Distribution


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
