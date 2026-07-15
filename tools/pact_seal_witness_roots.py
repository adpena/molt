#!/usr/bin/env python3
"""Regenerate the sealed pact-witness NumPy extension roots.

The pact Kernel A witness acceptance lane links source-recompiled NumPy
``.molt.wasm`` static-link artifacts through sealed extension roots under
``tmp/`` (see ``tools/proof_queue_pkg/pact.py::_pact_witness_native_roots``). Those roots
are gitignored, ephemeral build artifacts. Their extension manifests record the
``molt_c_api_version`` / ``abi_tag`` of the runtime they were produced against.

When the runtime C-API version advances (``include/molt/molt.h``
``MOLT_C_API_VERSION``), the previously sealed roots carry a stale ABI tag and
the witness build fails closed at native-artifact custody
("ABI mismatch: required N, manifest has M"). The fix is to re-run the real
``molt extension seal`` producer over each root so every manifest is re-derived
against the current runtime authority.

This recipe is the committed, deterministic regenerator so a fresh clone can
rebuild the abi-current sealed witness roots from the on-disk sealed artifacts
without hand-editing generated manifests:

- It discovers each witness root and every ``*.extension_manifest.json`` inside
  it (the same walk the external-native resolver performs).
- It checksum-verifies each declared ``.molt.wasm`` artifact against the
  manifest's ``extension_sha256`` *before* re-sealing, so the ABI re-stamp is
  proven to apply to the intended, intact binary rather than a stale or swapped
  one.
- It re-seals every module in place through ``molt.cli.extension_seal``, sealing
  secondary modules first and the root-level (primary) module last so the
  root ``extension_manifest.json`` continues to name the primary module. Seal
  re-stamps ``molt_c_api_version`` / ``abi_tag`` from the current runtime, and
  re-derives source capsule and C-API custody from the checksummed sources.
- It fails closed with a precise diagnostic if a root, manifest, or binary is
  missing, if a binary checksum does not match its manifest, or if any output
  manifest is not at the current runtime ABI.

The underlying ``.molt.wasm`` relocatable objects are unchanged: the Molt
extension ABI is a monotonically forward-compatible stable core, and the
NumPy 1->N C-API version bumps observed here were additive. An artifact
recorded at a newer major ABI than the current runtime is a genuine mismatch;
``molt extension seal`` refuses to re-stamp it downward and fails closed.

Usage::

    uv run --active --project . --python 3.12 \
        python tools/pact_seal_witness_roots.py [--check] [--root DIR ...]

``--check`` verifies that every witness-root manifest is already at the current
runtime ABI (and that binaries match their checksums) without rewriting; it
exits non-zero if regeneration is required. Without ``--check`` it regenerates
in place.
"""

from __future__ import annotations

import argparse
import json
import os
import sys
from pathlib import Path


def _repo_root() -> Path:
    return Path(__file__).resolve().parent.parent


# Ensure the in-tree ``molt`` package is importable when run directly.
_SRC = _repo_root() / "src"
if _SRC.is_dir() and str(_SRC) not in sys.path:
    sys.path.insert(0, str(_SRC))

from molt.cli.extension_manifest import (  # noqa: E402
    _CURRENT_MOLT_C_API_VERSION,
    _default_molt_c_api_version,
)
from molt.cli.external_native import _resolve_manifest_runtime_python_imports  # noqa: E402
from molt.cli.extension_seal import extension_seal  # noqa: E402
from molt.cli.file_hashing import _sha256_file  # noqa: E402
from molt.cli.source_extensions import (  # noqa: E402
    _manifest_source_plan_relocation_roots,
    _source_extension_relocation_roots,
    source_extension_manifest_source_path,
)
from molt.scientific_stack_versions import (  # noqa: E402
    resolve_scientific_stack,
    verify_cpython_abi_headers,
)

# Sibling tool: stages NumPy's full pure-Python subtree + build-generated
# modules into every witness sealed root so a re-seal leaves the witness import
# closure complete (see the module docstring).
if str(Path(__file__).resolve().parent) not in sys.path:
    sys.path.insert(0, str(Path(__file__).resolve().parent))
import pact_witness_numpy_python_closure as _numpy_python_closure  # noqa: E402


# The witness sealed roots, relative to the repo root. These mirror the primary
# candidates selected by tools/proof_queue_pkg/pact.py::_pact_witness_native_roots. Only
# roots that exist on disk are regenerated; missing roots are reported so a
# partial staging environment fails closed rather than silently skipping.
_DEFAULT_WITNESS_ROOTS: tuple[str, ...] = (
    "tmp/pact_numpy_multiarray_sealed_for_witness",
)

_ARTIFACT_MANIFEST_SUFFIX = ".extension_manifest.json"
_ROOT_MANIFEST_NAME = "extension_manifest.json"


class RegenError(Exception):
    """A fail-closed custody error while regenerating a witness root."""


def _load_manifest(path: Path) -> dict:
    try:
        data = json.loads(path.read_text(encoding="utf-8"))
    except (OSError, json.JSONDecodeError) as exc:
        raise RegenError(f"cannot read extension manifest {path}: {exc}") from exc
    if not isinstance(data, dict):
        raise RegenError(f"extension manifest is not a JSON object: {path}")
    return data


def _resolve_declared_artifact(manifest: dict, manifest_path: Path) -> Path:
    extension = manifest.get("extension")
    if not isinstance(extension, str) or not extension.strip():
        raise RegenError(f"manifest {manifest_path} has no 'extension' artifact path")
    candidate = Path(extension.strip())
    candidates = (
        (candidate,)
        if candidate.is_absolute()
        else (
            manifest_path.parent / candidate,
            manifest_path.parent / candidate.name,
        )
    )
    for cand in candidates:
        if cand.is_file():
            return cand.resolve()
    raise RegenError(
        f"manifest {manifest_path} declares extension artifact "
        f"'{extension}' which was not found"
    )


def _verify_artifact_checksum(manifest: dict, artifact_path: Path) -> None:
    declared = manifest.get("extension_sha256")
    if not isinstance(declared, str) or not declared.strip():
        raise RegenError(
            f"manifest for {artifact_path} has no extension_sha256 custody; "
            "cannot prove the artifact before re-stamping its ABI"
        )
    actual = _sha256_file(artifact_path)
    if actual != declared.strip():
        raise RegenError(
            f"artifact checksum mismatch for {artifact_path}: manifest declares "
            f"{declared.strip()} but bytes hash to {actual}. Rebuild the sealed "
            "root from the extension producer before re-stamping the ABI."
        )


def _artifact_manifests(root: Path) -> list[Path]:
    manifests: list[Path] = []
    for current_root, dirnames, filenames in os.walk(root):
        dirnames[:] = sorted(dirnames)
        for filename in sorted(filenames):
            if filename.endswith(_ARTIFACT_MANIFEST_SUFFIX):
                manifests.append(Path(current_root) / filename)
    return manifests


def _primary_module(root: Path) -> str | None:
    root_manifest_path = root / _ROOT_MANIFEST_NAME
    if not root_manifest_path.is_file():
        return None
    return _load_manifest(root_manifest_path).get("module")


def _ordered_manifests(root: Path) -> list[Path]:
    """Order per-artifact manifests so the primary module seals last.

    Seal overwrites the root-level ``extension_manifest.json`` with the module
    it is sealing, so sealing the primary module last keeps the root manifest
    naming the primary module, matching the original sealed topology.
    """
    manifests = _artifact_manifests(root)
    primary = _primary_module(root)
    if primary is None:
        return manifests

    def sort_key(path: Path) -> tuple[int, str]:
        module = _load_manifest(path).get("module")
        return (1 if module == primary else 0, str(path))

    return sorted(manifests, key=sort_key)


def _manifest_abi(manifest: dict) -> tuple[str | None, str | None]:
    abi = manifest.get("molt_c_api_version")
    tag = manifest.get("abi_tag")
    return (
        abi if isinstance(abi, str) else None,
        tag if isinstance(tag, str) else None,
    )


def _manifest_package(manifest: dict, manifest_path: Path) -> str:
    module = manifest.get("module")
    if isinstance(module, str) and module.strip():
        return module.strip().split(".", 1)[0]
    return manifest_path.parent.name


def _runtime_python_import_custody_problems(
    manifest: dict,
    manifest_path: Path,
) -> list[str]:
    _imports, errors = _resolve_manifest_runtime_python_imports(
        manifest,
        manifest_path=manifest_path,
        package=_manifest_package(manifest, manifest_path),
    )
    return [f"{manifest_path}: {error}" for error in errors]


def _object_source_expected_sha256(item: dict) -> str | None:
    value = item.get("source_sha256")
    if isinstance(value, str) and value.strip():
        return value.strip()
    return None


def _object_closure_source_problems(
    manifest: dict,
    manifest_path: Path,
) -> list[str]:
    problems: list[str] = []
    object_closure = manifest.get("object_closure")
    objects = (
        object_closure.get("objects") if isinstance(object_closure, dict) else None
    )
    if not isinstance(objects, list):
        return problems
    for index, item in enumerate(objects):
        if not isinstance(item, dict):
            continue
        source = item.get("source")
        if not isinstance(source, str) or not source.strip():
            continue
        source_path, errors = source_extension_manifest_source_path(
            source,
            manifest=manifest,
            manifest_path=manifest_path,
            expected_sha256=_object_source_expected_sha256(item),
        )
        if errors:
            problems.extend(f"{manifest_path}: {error}" for error in errors)
            continue
        if source_path is None:
            problems.append(
                f"{manifest_path}: object_closure.objects[{index}].source "
                f"does not resolve through source_plan roots: {source}"
            )
            continue
        if Path(source).expanduser().is_absolute():
            problems.append(
                f"{manifest_path}: object_closure.objects[{index}].source "
                f"is absolute; re-seal must relativize it to source_plan roots: "
                f"{source}"
            )
    return problems


def _source_plan_relative_source(
    *,
    manifest: dict,
    manifest_path: Path,
    raw_source: str,
    resolved_source: Path,
) -> str | None:
    raw_path = Path(raw_source).expanduser()
    roots = _manifest_source_plan_relocation_roots(manifest)
    if not roots:
        return None
    if not raw_path.is_absolute():
        return raw_path.as_posix()
    for root in roots:
        try:
            return raw_path.relative_to(root).as_posix()
        except ValueError:
            pass
    resolved = resolved_source.resolve()
    for logical_root in roots:
        for relocated_root in _source_extension_relocation_roots(
            logical_root,
            manifest_path=manifest_path,
        ):
            try:
                return resolved.relative_to(relocated_root).as_posix()
            except ValueError:
                continue
    return None


def _relocate_source_plan_root(
    *,
    manifest: dict,
    manifest_path: Path,
    resolved_source: Path,
) -> bool:
    source_plan = manifest.get("source_plan")
    if not isinstance(source_plan, dict):
        return False
    resolved = resolved_source.resolve()
    for field in ("source_root", "build_root"):
        raw_root = source_plan.get(field)
        if not isinstance(raw_root, str) or not raw_root.strip():
            continue
        logical_root = Path(raw_root).expanduser()
        try:
            resolved.relative_to(logical_root.resolve())
            return False
        except ValueError:
            pass
        for relocated_root in _source_extension_relocation_roots(
            logical_root,
            manifest_path=manifest_path,
        ):
            try:
                resolved.relative_to(relocated_root.resolve())
            except ValueError:
                continue
            source_plan[field] = str(relocated_root.resolve())
            return True
    return False


def _relativize_object_closure_sources(manifest_path: Path) -> bool:
    manifest = _load_manifest(manifest_path)
    object_closure = manifest.get("object_closure")
    objects = (
        object_closure.get("objects") if isinstance(object_closure, dict) else None
    )
    if not isinstance(objects, list):
        return False
    changed = False
    for index, item in enumerate(objects):
        if not isinstance(item, dict):
            continue
        source = item.get("source")
        if not isinstance(source, str) or not source.strip():
            continue
        source_path, errors = source_extension_manifest_source_path(
            source,
            manifest=manifest,
            manifest_path=manifest_path,
            expected_sha256=_object_source_expected_sha256(item),
        )
        if errors or source_path is None:
            detail = "; ".join(errors) if errors else source
            raise RegenError(
                f"{manifest_path}: cannot resolve "
                f"object_closure.objects[{index}].source before relativizing: "
                f"{detail}"
            )
        relative_source = _source_plan_relative_source(
            manifest=manifest,
            manifest_path=manifest_path,
            raw_source=source,
            resolved_source=source_path,
        )
        if relative_source is None:
            raise RegenError(
                f"{manifest_path}: object_closure.objects[{index}].source "
                f"is outside source_plan roots and cannot be made relocatable: "
                f"{source}"
            )
        if source != relative_source:
            item["source"] = relative_source
            changed = True
        if _relocate_source_plan_root(
            manifest=manifest,
            manifest_path=manifest_path,
            resolved_source=source_path,
        ):
            changed = True
    if changed:
        manifest_path.write_text(
            json.dumps(manifest, indent=2, sort_keys=True) + "\n",
            encoding="utf-8",
        )
    return changed


def _relativize_root_object_closure_sources(root: Path) -> None:
    manifest_paths = _artifact_manifests(root)
    root_manifest = root / _ROOT_MANIFEST_NAME
    if root_manifest.is_file():
        manifest_paths.append(root_manifest)
    for manifest_path in manifest_paths:
        _relativize_object_closure_sources(manifest_path)


def _check_root(root: Path, expected_abi: str, expected_tag: str) -> list[str]:
    problems: list[str] = []
    manifests = _artifact_manifests(root)
    root_manifest = root / _ROOT_MANIFEST_NAME
    if root_manifest.is_file():
        manifests.append(root_manifest)
    if not manifests:
        return [f"{root}: no extension manifests found"]
    for manifest_path in manifests:
        manifest = _load_manifest(manifest_path)
        abi, tag = _manifest_abi(manifest)
        if abi != expected_abi or tag != expected_tag:
            problems.append(
                f"{manifest_path}: molt_c_api_version={abi} abi_tag={tag} "
                f"(expected {expected_abi} / {expected_tag})"
            )
        problems.extend(
            _runtime_python_import_custody_problems(manifest, manifest_path)
        )
        problems.extend(_object_closure_source_problems(manifest, manifest_path))
        # Only per-artifact manifests declare a resealable artifact path we can
        # checksum; the root manifest mirrors the primary artifact manifest.
        if manifest_path.name.endswith(_ARTIFACT_MANIFEST_SUFFIX):
            artifact = _resolve_declared_artifact(manifest, manifest_path)
            _verify_artifact_checksum(manifest, artifact)
    return problems


def _regenerate_root(root: Path, expected_abi: str, expected_tag: str) -> None:
    manifests = _ordered_manifests(root)
    if not manifests:
        raise RegenError(f"{root}: no per-artifact extension manifests to re-seal")
    # Repair relocatable source custody before extension_seal validates it. The
    # seal then recomputes derived import/closure facts from the relative path.
    _relativize_root_object_closure_sources(root)
    for manifest_path in manifests:
        manifest = _load_manifest(manifest_path)
        artifact = _resolve_declared_artifact(manifest, manifest_path)
        _verify_artifact_checksum(manifest, artifact)
        rc = extension_seal(
            path=str(manifest_path),
            out_dir=str(root),
            json_output=False,
        )
        if rc != 0:
            raise RegenError(
                f"molt extension seal failed (rc={rc}) for {manifest_path}"
            )
    problems = _check_root(root, expected_abi, expected_tag)
    if problems:
        raise RegenError(
            "re-sealed root is still not at the current ABI:\n  "
            + "\n  ".join(problems)
        )


def main(argv: list[str] | None = None) -> int:
    stack = resolve_scientific_stack()
    verify_cpython_abi_headers(stack=stack, repo_root=_repo_root())
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument(
        "--check",
        action="store_true",
        help="verify roots are at the current ABI without rewriting",
    )
    parser.add_argument(
        "--root",
        action="append",
        dest="roots",
        default=None,
        help="explicit witness root (repeatable); defaults to the known roots",
    )
    args = parser.parse_args(argv)

    repo_root = _repo_root()
    expected_abi = _default_molt_c_api_version(repo_root)
    if not expected_abi:
        expected_abi = _CURRENT_MOLT_C_API_VERSION
    try:
        expected_major = int(expected_abi.split(".", 1)[0])
    except (ValueError, IndexError):
        print(
            f"cannot determine current runtime C-API major from {expected_abi!r}",
            file=sys.stderr,
        )
        return 2
    expected_tag = f"molt_abi{expected_major}"

    raw_roots = args.roots if args.roots else list(_DEFAULT_WITNESS_ROOTS)
    roots: list[Path] = []
    missing: list[str] = []
    for raw in raw_roots:
        path = Path(raw)
        if not path.is_absolute():
            path = repo_root / path
        if path.is_dir():
            roots.append(path)
        else:
            missing.append(str(path))

    if not roots:
        print(
            "no witness sealed roots found on disk; nothing to regenerate. "
            "Stage the pact witness roots first (see collab/pact/STATUS.md).",
            file=sys.stderr,
        )
        for path in missing:
            print(f"  missing: {path}", file=sys.stderr)
        return 1

    print(
        f"verified scientific stack: {stack.tuple_label}; "
        f"current runtime ABI: {expected_abi} ({expected_tag})"
    )
    for path in missing:
        print(f"  note: witness root not present, skipping: {path}")

    exit_code = 0
    for root in roots:
        try:
            if args.check:
                problems = _check_root(root, expected_abi, expected_tag)
                if problems:
                    print(f"STALE {root}:")
                    for problem in problems:
                        print(f"  {problem}")
                    exit_code = 1
                else:
                    print(f"OK    {root} (all manifests at {expected_tag})")
            else:
                _regenerate_root(root, expected_abi, expected_tag)
                print(f"SEALED {root} -> {expected_tag}")
        except RegenError as exc:
            print(f"FAIL  {root}: {exc}", file=sys.stderr)
            exit_code = 2

    # An explicit root request is intentionally scoped to those roots. Default
    # NumPy closure staging is a separate side effect and must not run while a
    # caller is inspecting or repairing an isolated root.
    if args.roots is not None:
        return exit_code

    # NumPy's pure-Python submodules (and its build-generated version.py/
    # __config__.py) do not travel with a source-derived C-ext re-seal; stage
    # NumPy's full importable subtree from the off-the-shelf checkout into every
    # sealed root so the witness import closure is complete and a re-seal cannot
    # leave any submodule out.
    try:
        if args.check:
            problems = _numpy_python_closure.check(repo_root)
            if problems:
                print(f"STALE {repo_root} (numpy pure-Python closure):")
                for problem in problems[:20]:
                    print(f"  {problem}")
                if len(problems) > 20:
                    print(f"  ... (+{len(problems) - 20} more)")
                exit_code = exit_code or 1
            else:
                print("OK    numpy pure-Python closure staged + current")
        else:
            written = _numpy_python_closure.stage(repo_root)
            print(f"STAGED {len(written)} numpy module files across witness roots")
    except (
        _numpy_python_closure.ClosureStagingError,
        _numpy_python_closure._generated.GeneratedModuleError,
    ) as exc:
        print(f"FAIL  numpy pure-Python closure: {exc}", file=sys.stderr)
        exit_code = 2

    return exit_code


if __name__ == "__main__":
    raise SystemExit(main())
