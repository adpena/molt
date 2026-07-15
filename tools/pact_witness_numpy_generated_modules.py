#!/usr/bin/env python3
"""Materialize NumPy's build-generated Python modules for the pact witness.

Root cause this closes (M58 numpy-seal / witness-closure completeness):

NumPy ships two Python modules that do **not** exist in its source tree; the
meson build generates them at build time and installs them into the package:

- ``numpy/version.py`` -- written by ``numpy/_build_utils/gitversion.py``. It is
  the authority for ``numpy.__version__`` and is imported unconditionally by
  ``numpy/__init__.py`` (``from . import version``) and by
  ``numpy/_core/__init__.py`` (``from numpy.version import version``).
- ``numpy/__config__.py`` -- configured by meson from ``numpy/__config__.py.in``.
  ``numpy/__init__.py`` imports ``from numpy.__config__ import show_config`` and
  *fails closed* (raises ``ImportError``) when it is a ``ModuleNotFoundError``
  for ``numpy.__config__``.

The pact witness stages NumPy from source-derived module roots (the sealed
extension root under ``tmp/pact_numpy_multiarray_sealed_for_witness`` and the
off-the-shelf checkout under ``bench/friends/repos/numpy_off_the_shelf``).
Because those roots carry NumPy's *source* tree, the two build-generated
modules are absent, and the source-derived support-file closure silently drops
them (a package-relative import whose candidate does not resolve on disk is
treated as "not a module"). At witness runtime ``from . import version`` then
raises ``ImportError: cannot import name 'version' from 'numpy'`` and, once that
is provided, ``from numpy.__config__ import show_config`` would raise next.

This tool is the class-level generator: it re-derives both modules from NumPy's
**own** authoritative producers and materializes them into every witness NumPy
module root so the support-file closure includes them and the witness can
import ``numpy`` end to end. It is idempotent and runs (a) standalone and (b)
from ``tools/pact_seal_witness_roots.py`` after a re-seal, so a re-seal can
never wipe the generated modules back out.

- ``version.py`` is produced by executing NumPy's committed
  ``numpy/_build_utils/gitversion.py --write`` in the checkout. The version
  string is sourced from NumPy's ``pyproject.toml`` and the git revision from
  the checkout HEAD -- never a hand-typed literal.
- ``__config__.py`` is copied from the real meson-generated build output when
  present (the fully substituted module a NumPy wasm build emits). No fabricated
  compiler metadata is synthesized; if no authoritative build output is found
  the tool fails closed and names the meson build root to regenerate.

Usage::

    python tools/pact_witness_numpy_generated_modules.py [--check] [--repo-root DIR]
"""

from __future__ import annotations

import argparse
import subprocess
import sys
import tempfile
from pathlib import Path


class GeneratedModuleError(Exception):
    """A fail-closed error while materializing a build-generated module."""


def _repo_root() -> Path:
    return Path(__file__).resolve().parent.parent


# NumPy source checkout, relative to the repo root.
_NUMPY_SOURCE_REL = "bench/friends/repos/numpy_off_the_shelf"

# Witness NumPy package roots that must carry the generated modules. Each entry
# is a directory that (when it exists) contains a ``numpy/`` package the witness
# stages/resolves from. Mirrors the numpy roots selected by
# tools/proof_queue_pkg/pact.py::_pact_witness_native_roots.
_WITNESS_NUMPY_PACKAGE_PARENTS = (
    "tmp/pact_numpy_multiarray_sealed_for_witness",
    "tmp/pact_numpy_multiarray_sealed_axiserror",
    _NUMPY_SOURCE_REL,
)

# Candidate locations of a real meson-generated ``numpy/__config__.py``.
_CONFIG_BUILD_CANDIDATES = (
    "tmp/pact_numpy_multiarray_meson_wasm_build_generated/numpy/__config__.py",
    "tmp/pact_numpy_multiarray_meson_wasm_build/numpy/__config__.py",
)

_GENERATED_MODULES = ("version.py", "__config__.py")


def _numpy_pyproject_version(numpy_source_root: Path) -> str:
    """Authoritative NumPy version string, read from NumPy's pyproject.toml."""
    pyproject = numpy_source_root / "pyproject.toml"
    try:
        lines = pyproject.read_text(encoding="utf-8").splitlines()
    except OSError as exc:
        raise GeneratedModuleError(
            f"cannot read NumPy pyproject.toml at {pyproject}: {exc}"
        ) from exc
    for line in lines:
        if line.startswith("version ="):
            return line.split("=", 1)[1].strip().strip("\"'")
    raise GeneratedModuleError(
        f"no 'version =' line in NumPy pyproject.toml at {pyproject}"
    )


def _generate_version_module(numpy_source_root: Path) -> str:
    """Run NumPy's own gitversion.py generator and return version.py content."""
    gitversion = (
        numpy_source_root / "numpy" / "_build_utils" / "gitversion.py"
    )
    if not gitversion.is_file():
        raise GeneratedModuleError(
            f"NumPy gitversion generator not found at {gitversion}; "
            "cannot derive numpy/version.py from NumPy's own authority"
        )
    with tempfile.TemporaryDirectory() as tmpdir:
        out_path = Path(tmpdir) / "version.py"
        proc = subprocess.run(
            [sys.executable, str(gitversion), "--write", str(out_path)],
            capture_output=True,
            text=True,
            cwd=str(numpy_source_root),
        )
        if proc.returncode != 0:
            raise GeneratedModuleError(
                f"NumPy gitversion.py failed (rc={proc.returncode}): "
                f"{proc.stderr.strip() or proc.stdout.strip()}"
            )
        try:
            content = out_path.read_text(encoding="utf-8")
        except OSError as exc:
            raise GeneratedModuleError(
                f"NumPy gitversion.py did not write {out_path}: {exc}"
            ) from exc
    expected_version = _numpy_pyproject_version(numpy_source_root)
    if f'version = "{expected_version}"' not in content:
        raise GeneratedModuleError(
            "generated numpy/version.py does not carry the pyproject version "
            f"{expected_version!r}; refusing to materialize an inconsistent "
            "version module"
        )
    return content


def _generate_config_module(repo_root: Path) -> tuple[str, Path]:
    """Locate the real meson-generated numpy/__config__.py and return it."""
    for rel in _CONFIG_BUILD_CANDIDATES:
        candidate = repo_root / rel
        if candidate.is_file():
            content = candidate.read_text(encoding="utf-8")
            if "@" in content and "@C_COMP@" in content:
                raise GeneratedModuleError(
                    f"{candidate} still has unsubstituted meson @VARS@; it is a "
                    "template, not a generated build output"
                )
            return content, candidate
    raise GeneratedModuleError(
        "no authoritative meson-generated numpy/__config__.py found; searched: "
        + ", ".join(str(repo_root / rel) for rel in _CONFIG_BUILD_CANDIDATES)
        + ". Regenerate NumPy's wasm meson build so __config__.py exists, then "
        "re-run; refusing to fabricate build/compiler metadata."
    )


def _witness_numpy_package_dirs(repo_root: Path) -> list[Path]:
    """Existing ``numpy/`` package dirs across the witness module roots."""
    dirs: list[Path] = []
    for parent_rel in _WITNESS_NUMPY_PACKAGE_PARENTS:
        numpy_dir = repo_root / parent_rel / "numpy"
        if numpy_dir.is_dir():
            dirs.append(numpy_dir)
    return dirs


def _sources(repo_root: Path) -> dict[str, str]:
    numpy_source_root = repo_root / _NUMPY_SOURCE_REL
    if not numpy_source_root.is_dir():
        raise GeneratedModuleError(
            f"NumPy source checkout not found at {numpy_source_root}; provision "
            "the off-the-shelf NumPy checkout before materializing generated "
            "modules"
        )
    version_src = _generate_version_module(numpy_source_root)
    config_src, config_origin = _generate_config_module(repo_root)
    return {
        "version.py": version_src,
        "__config__.py": config_src,
        "__config__.origin": str(config_origin),
    }


def materialize(repo_root: Path) -> list[Path]:
    """Write the generated modules into every witness NumPy package dir."""
    sources = _sources(repo_root)
    numpy_dirs = _witness_numpy_package_dirs(repo_root)
    if not numpy_dirs:
        raise GeneratedModuleError(
            "no witness NumPy package roots present on disk; nothing to "
            "materialize. Stage the pact witness roots first."
        )
    written: list[Path] = []
    for numpy_dir in numpy_dirs:
        for name in _GENERATED_MODULES:
            target = numpy_dir / name
            content = sources[name]
            if not target.is_file() or target.read_text(encoding="utf-8") != content:
                target.write_text(content, encoding="utf-8")
            written.append(target)
    return written


def check(repo_root: Path) -> list[str]:
    """Return a list of problems; empty means every root is complete + current."""
    problems: list[str] = []
    try:
        sources = _sources(repo_root)
    except GeneratedModuleError as exc:
        return [str(exc)]
    numpy_dirs = _witness_numpy_package_dirs(repo_root)
    if not numpy_dirs:
        return ["no witness NumPy package roots present on disk"]
    for numpy_dir in numpy_dirs:
        for name in _GENERATED_MODULES:
            target = numpy_dir / name
            if not target.is_file():
                problems.append(f"missing generated module: {target}")
                continue
            if target.read_text(encoding="utf-8") != sources[name]:
                problems.append(
                    f"stale generated module (does not match NumPy's authority): "
                    f"{target}"
                )
    return problems


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument(
        "--check",
        action="store_true",
        help="verify the generated modules are present + current without writing",
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
                print("STALE numpy generated modules:")
                for problem in problems:
                    print(f"  {problem}")
                return 1
            print("OK numpy build-generated modules present + current")
            return 0
        written = materialize(repo_root)
        for path in written:
            print(f"materialized {path}")
        return 0
    except GeneratedModuleError as exc:
        print(f"FAIL {exc}", file=sys.stderr)
        return 2


if __name__ == "__main__":
    raise SystemExit(main())
