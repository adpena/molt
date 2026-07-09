#!/usr/bin/env python3
"""Stage NumPy's full pure-Python subtree into the pact witness sealed roots.

Root cause this closes (M56/M58 numpy-custody closure completeness):

The pact witness admits NumPy as an *external static package*: the sealed root
under ``tmp/pact_numpy_multiarray_sealed_for_witness`` owns NumPy's compiled
``_multiarray_umath.molt.wasm`` + its extension manifest, and the build resolves
NumPy's pure-Python support surface as the import-driven closure of that sealed
root (see ``molt.cli.external_native``). ``package_source_root`` for that closure
is the sealed root itself; the off-the-shelf NumPy checkout is a source-plan/
module root but is *not* consulted for the external static package's support
files (it owns no native artifact for NumPy).

The sealed root was staged with only the C-ext-adjacent files
(``numpy/__init__.py`` + ``numpy/_core/__init__.py``). Every pure-Python
submodule ``numpy/__init__.py`` imports -- ``_expired_attrs_2_0`` (L94),
``_globals`` (L95), ``lib``/``matrixlib`` (L457), and their transitive closure --
lives only in the off-the-shelf source tree, so the closure BFS cannot resolve
them (a package-relative import whose candidate is absent from the sealed root is
dropped) and each one fail-closes at witness runtime with
``ModuleNotFoundError: No module named 'numpy.<sub>'``.

This tool is the systematic, one-pass fix: it mirrors NumPy's entire importable
pure-Python subtree from the off-the-shelf source checkout into every witness
sealed root, so the *existing* import-driven support-file closure can resolve and
stage NumPy's full transitive Python import closure. It does NOT hand-pick
submodules (that would be whack-a-mole) and it does NOT loosen any import; it
provides the modules NumPy's ``__init__`` genuinely imports.

It also materializes NumPy's build-generated modules (``version.py``,
``__config__.py``) via ``pact_witness_numpy_generated_modules`` so the sealed
tree is a complete NumPy package.

The C-extension artifacts already staged in the sealed root
(``*.molt.wasm`` + ``*.extension_manifest.json``) and the generated modules are
never overwritten by the source mirror.

Usage::

    python tools/pact_witness_numpy_python_closure.py [--check] [--repo-root DIR]
"""

from __future__ import annotations

import argparse
import sys
from pathlib import Path

if str(Path(__file__).resolve().parent) not in sys.path:
    sys.path.insert(0, str(Path(__file__).resolve().parent))
import pact_witness_numpy_generated_modules as _generated  # noqa: E402


class ClosureStagingError(Exception):
    """A fail-closed error while staging NumPy's pure-Python closure."""


def _repo_root() -> Path:
    return Path(__file__).resolve().parent.parent


# NumPy source checkout, relative to the repo root (the pure-Python authority).
_NUMPY_SOURCE_REL = _generated._NUMPY_SOURCE_REL

# Witness sealed roots that must carry NumPy's full pure-Python subtree. The
# off-the-shelf checkout is the SOURCE, not a target.
_WITNESS_SEALED_PARENTS = tuple(
    parent
    for parent in _generated._WITNESS_NUMPY_PACKAGE_PARENTS
    if parent != _NUMPY_SOURCE_REL
)

# Directory segments never imported at witness runtime; excluded to keep the
# sealed tree to NumPy's importable surface. ``tests`` packages are unit-test
# trees that ``numpy/__init__`` never imports.
_EXCLUDED_DIR_SEGMENTS = frozenset({"tests"})

# Files the source mirror must never overwrite in a sealed root: the compiled
# extension artifacts + their manifests, and the build-generated modules (owned
# by pact_witness_numpy_generated_modules). NumPy's source tree does not contain
# these names as ``.py`` sources, but guard explicitly so the mirror stays a
# pure source overlay.
_MIRROR_PROTECTED_NAMES = frozenset({"version.py", "__config__.py"})


def _numpy_source_root(repo_root: Path) -> Path:
    root = repo_root / _NUMPY_SOURCE_REL / "numpy"
    if not root.is_dir():
        raise ClosureStagingError(
            f"NumPy source checkout not found at {root}; provision the "
            "off-the-shelf NumPy checkout before staging the witness closure"
        )
    return root


def _importable_python_sources(numpy_source: Path) -> list[Path]:
    """Every importable ``*.py`` under NumPy's source tree, tests excluded."""
    sources: list[Path] = []
    for path in numpy_source.rglob("*.py"):
        rel_parts = path.relative_to(numpy_source).parts
        if any(part in _EXCLUDED_DIR_SEGMENTS for part in rel_parts[:-1]):
            continue
        sources.append(path)
    return sources


def _sealed_numpy_dirs(repo_root: Path) -> list[Path]:
    dirs: list[Path] = []
    for parent in _WITNESS_SEALED_PARENTS:
        numpy_dir = repo_root / parent / "numpy"
        if numpy_dir.is_dir():
            dirs.append(numpy_dir)
    return dirs


def _mirror_into(numpy_source: Path, sealed_numpy: Path) -> list[Path]:
    written: list[Path] = []
    for source_path in _importable_python_sources(numpy_source):
        rel = source_path.relative_to(numpy_source)
        if rel.name in _MIRROR_PROTECTED_NAMES:
            continue
        target = sealed_numpy / rel
        content = source_path.read_bytes()
        if not target.is_file() or target.read_bytes() != content:
            target.parent.mkdir(parents=True, exist_ok=True)
            target.write_bytes(content)
        written.append(target)
    return written


def stage(repo_root: Path) -> list[Path]:
    """Mirror NumPy's pure-Python subtree + generated modules into every root."""
    numpy_source = _numpy_source_root(repo_root)
    sealed_dirs = _sealed_numpy_dirs(repo_root)
    if not sealed_dirs:
        raise ClosureStagingError(
            "no witness sealed NumPy roots present on disk; stage the pact "
            "witness roots first."
        )
    written: list[Path] = []
    for sealed_numpy in sealed_dirs:
        written.extend(_mirror_into(numpy_source, sealed_numpy))
    # Build-generated modules (version.py/__config__.py) come from NumPy's own
    # authorities, layered on top of the source mirror.
    written.extend(_generated.materialize(repo_root))
    return written


def check(repo_root: Path) -> list[str]:
    """Return closure-completeness problems; empty means every root is complete."""
    problems: list[str] = []
    try:
        numpy_source = _numpy_source_root(repo_root)
    except ClosureStagingError as exc:
        return [str(exc)]
    sealed_dirs = _sealed_numpy_dirs(repo_root)
    if not sealed_dirs:
        return ["no witness sealed NumPy roots present on disk"]
    source_rels = [
        p.relative_to(numpy_source)
        for p in _importable_python_sources(numpy_source)
        if p.name not in _MIRROR_PROTECTED_NAMES
    ]
    for sealed_numpy in sealed_dirs:
        for rel in source_rels:
            target = sealed_numpy / rel
            if not target.is_file():
                problems.append(f"missing staged module: {target}")
                continue
            if target.read_bytes() != (numpy_source / rel).read_bytes():
                problems.append(f"stale staged module (source drift): {target}")
    problems.extend(_generated.check(repo_root))
    return problems


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument(
        "--check",
        action="store_true",
        help="verify every sealed root carries NumPy's full pure-Python closure",
    )
    parser.add_argument(
        "--repo-root",
        type=Path,
        default=None,
        help="repo root to operate on (defaults to this checkout)",
    )
    args = parser.parse_args(argv)
    repo_root = (args.repo_root or _repo_root()).resolve()

    try:
        if args.check:
            problems = check(repo_root)
            if problems:
                print(f"STALE numpy witness closure ({len(problems)} problems):")
                for problem in problems[:20]:
                    print(f"  {problem}")
                if len(problems) > 20:
                    print(f"  ... (+{len(problems) - 20} more)")
                return 1
            print("OK numpy pure-Python closure staged + current in every root")
            return 0
        written = stage(repo_root)
        print(f"staged {len(written)} numpy module files across witness roots")
        return 0
    except (ClosureStagingError, _generated.GeneratedModuleError) as exc:
        print(f"FAIL {exc}", file=sys.stderr)
        return 2


if __name__ == "__main__":
    raise SystemExit(main())
