#!/usr/bin/env python3
"""Artifact-effect poison gate — fail LOUD when a built wasm ships a stub.

Apparatus (2026-07-10). A capability being *configured* (archive resolves, flag
set) does not prove it is *effective* in the built artifact. This gate scans a
compiled wasm (runtime and/or app) for byte-markers that prove a degraded/stub
capability linked in — the exact "resolve != effective" (M34) class that
otherwise surfaces ~minutes later as an opaque ``RuntimeError: unreachable`` at
runtime, and that a human (or an LLM orchestrator) can wrongly mark "done" on a
proxy signal. Markers are DATA STRINGS (abort/diagnostic messages) so they
survive symbol stripping.

Usage:
    python tools/artifact_poison_gate.py <wasm> [<wasm> ...]
    python tools/artifact_poison_gate.py --registry PATH <wasm> ...

Exit codes:
    0  clean — no poison markers present
    2  POISON PRESENT — one or more built artifacts ship a stub capability
    3  usage/config error (missing file, unreadable registry)

Designed to be called from the witness acceptance build path (fail before the
run) and standalone in CI.
"""
from __future__ import annotations

import argparse
from pathlib import Path
import sys

try:
    import tomllib  # py3.11+
except ModuleNotFoundError:  # pragma: no cover - py<3.11 fallback
    import tomli as tomllib  # type: ignore[no-redef]

_DEFAULT_REGISTRY = Path(__file__).resolve().parent / "artifact_poison_registry.toml"


class PoisonHit:
    __slots__ = ("artifact", "marker_id", "capability", "why", "fix")

    def __init__(self, artifact: Path, marker_id: str, entry: dict[str, str]) -> None:
        self.artifact = artifact
        self.marker_id = marker_id
        self.capability = entry.get("capability", "?")
        self.why = entry.get("why", "")
        self.fix = entry.get("fix", "")


def load_markers(registry_path: Path) -> dict[str, dict[str, str]]:
    data = tomllib.loads(registry_path.read_text(encoding="utf-8"))
    markers = data.get("markers", {})
    if not isinstance(markers, dict):
        raise ValueError(f"{registry_path}: [markers] must be a table")
    for mid, entry in markers.items():
        if not isinstance(entry, dict) or "bytes" not in entry:
            raise ValueError(f"{registry_path}: marker '{mid}' needs a `bytes` field")
    return markers


def scan_artifact(path: Path, markers: dict[str, dict[str, str]]) -> list[PoisonHit]:
    """Return every poison marker whose byte-string is present in ``path``."""
    blob = path.read_bytes()
    hits: list[PoisonHit] = []
    for mid, entry in markers.items():
        needle = entry["bytes"].encode("utf-8")
        if needle in blob:
            hits.append(PoisonHit(path, mid, entry))
    return hits


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("wasm", nargs="+", type=Path, help="wasm artifact(s) to scan")
    parser.add_argument(
        "--registry",
        type=Path,
        default=_DEFAULT_REGISTRY,
        help="poison registry TOML (default: tools/artifact_poison_registry.toml)",
    )
    args = parser.parse_args(argv)

    try:
        markers = load_markers(args.registry)
    except (OSError, ValueError) as exc:
        print(f"artifact_poison_gate: cannot load registry: {exc}", file=sys.stderr)
        return 3

    missing = [p for p in args.wasm if not p.is_file()]
    if missing:
        for p in missing:
            print(f"artifact_poison_gate: no such artifact: {p}", file=sys.stderr)
        return 3

    all_hits: list[PoisonHit] = []
    for path in args.wasm:
        all_hits.extend(scan_artifact(path, markers))

    if not all_hits:
        names = ", ".join(p.name for p in args.wasm)
        print(f"artifact_poison_gate: PASS — no stub markers in {names} "
              f"({len(markers)} markers checked)")
        return 0

    print("artifact_poison_gate: FAIL — a built artifact ships a stub capability "
          "(configured != effective, M34):", file=sys.stderr)
    for hit in all_hits:
        print(f"\n  POISON: {hit.marker_id}", file=sys.stderr)
        print(f"    artifact:   {hit.artifact}", file=sys.stderr)
        print(f"    capability: {hit.capability}", file=sys.stderr)
        if hit.why:
            print(f"    why:        {hit.why}", file=sys.stderr)
        if hit.fix:
            print(f"    fix:        {hit.fix}", file=sys.stderr)
    print("\nA resolving-config check is NOT enough; the stub is in the BUILT wasm. "
          "Do not mark the capability done until this gate passes.", file=sys.stderr)
    return 2


if __name__ == "__main__":
    raise SystemExit(main())
