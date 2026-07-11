#!/usr/bin/env python3
"""Verify a fresh numpy ``_multiarray_umath`` sealed root satisfies the witness.

Deliverable gate for regenerating the stale
``pact_numpy_multiarray_sealed_for_witness`` seal. Asserts, against a sealed
``extension_manifest.json``:

  1. ``runtime_python_import_modules`` is present AND non-empty.
  2. Every ``object_closure`` object ``source`` path resolves on THIS host.
  3. The ``source_plan`` build_root / source_root / compile_commands resolve.

Prints a compact PASS/FAIL report. Exit 0 on PASS, 1 on FAIL.

Usage::

    python tools/verify_numpy_seal.py --manifest <sealed_dir_or_manifest.json>
"""

from __future__ import annotations

import argparse
import json
from pathlib import Path

from molt.cli.source_extensions import SourceExtensionManifestSourceResolver
from molt.scientific_stack_versions import (
    attest_numpy_witness_seal,
    numpy_witness_seal_root,
    resolve_scientific_stack,
    verify_cpython_abi_headers,
    verify_source_checkout,
)


def _load(path: Path) -> tuple[dict, Path]:
    if path.is_dir():
        path = path / "extension_manifest.json"
    return json.loads(path.read_text(encoding="utf-8")), path


def main() -> int:
    stack = resolve_scientific_stack()
    verify_cpython_abi_headers(stack=stack)
    ap = argparse.ArgumentParser()
    ap.add_argument("--manifest", type=Path)
    args = ap.parse_args()

    requested = args.manifest or numpy_witness_seal_root(stack=stack)
    manifest, mpath = _load(requested)
    seal_root = mpath.parent
    print(f"[verify] manifest: {mpath}")
    print(f"[verify] module:   {manifest.get('module') or manifest.get('name')}")

    failures: list[str] = []

    try:
        effective_version = attest_numpy_witness_seal(seal_root, stack=stack)
        print(
            f"[verify] NumPy version attestation: configured={stack.numpy} "
            f"effective={effective_version}"
        )
    except ValueError as exc:
        failures.append(str(exc))

    # (1) runtime_python_import_modules present + non-empty.
    rpim = manifest.get("runtime_python_import_modules")
    if not rpim:
        failures.append(f"runtime_python_import_modules missing/empty (value={rpim!r})")
    else:
        print(f"[verify] runtime_python_import_modules: {len(rpim)} entries")
        for m in rpim[:8]:
            print(f"           - {m}")
        if len(rpim) > 8:
            print(f"           ... (+{len(rpim) - 8} more)")

    # (2) every object_closure object source resolves.
    oc = manifest.get("object_closure") or {}
    objs = oc.get("objects") or []
    source_resolver = SourceExtensionManifestSourceResolver(
        manifest=manifest,
        manifest_path=mpath,
    )
    missing = []
    checked = 0
    for o in objs:
        src = o.get("source") if isinstance(o, dict) else None
        if not src:
            continue
        checked += 1
        source_sha256 = o.get("source_sha256") if isinstance(o, dict) else None
        source_path, source_errors = source_resolver.resolve(
            src,
            expected_sha256=(
                source_sha256.strip()
                if isinstance(source_sha256, str) and source_sha256.strip()
                else None
            ),
        )
        if source_errors:
            missing.extend(source_errors)
        elif source_path is None:
            missing.append(src)
        elif Path(src).expanduser().is_absolute():
            missing.append(
                f"object_closure source is absolute, not source_plan-relative: {src}"
            )
    print(f"[verify] object_closure objects: {len(objs)} (with source: {checked})")
    if missing:
        failures.append(
            f"{len(missing)} object_closure sources do not resolve on this host"
        )
        for s in missing[:5]:
            print(f"           MISSING: {s}")
    else:
        print(f"[verify] all {checked} object_closure sources resolve")

    # (3) source_plan roots resolve.
    sp = manifest.get("source_plan") or {}
    source_root = sp.get("source_root")
    if source_root:
        try:
            verify_source_checkout("numpy", Path(source_root), stack=stack)
        except ValueError as exc:
            failures.append(str(exc))
    for key in ("build_root", "source_root", "compile_commands"):
        val = sp.get(key)
        if val and not Path(val).expanduser().exists():
            failures.append(f"source_plan.{key} does not resolve: {val}")
            print(f"[verify] source_plan.{key}: MISSING {val}")
        elif val:
            print(f"[verify] source_plan.{key}: OK {val}")

    print()
    if failures:
        print("[verify] RESULT: FAIL")
        for f in failures:
            print(f"  - {f}")
        return 1
    print("[verify] RESULT: PASS")
    print("  - runtime_python_import_modules present + non-empty")
    print("  - every object_closure source resolves on this host")
    print("  - source_plan roots resolve")
    print(f"  - configured NumPy {stack.numpy} equals effective sealed version")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
