#!/usr/bin/env python3
"""Materialize SciPy's build-generated Python modules for the pact witness.

Root cause this closes (E1 witness / scipy-seal closure completeness -- the
SciPy analogue of ``pact_witness_numpy_generated_modules``):

SciPy ships two Python modules that do **not** exist in its source tree; the
meson build generates them at build time and installs them into the package:

- ``scipy/version.py`` -- written by SciPy's committed ``tools/gitversion.py``
  (``configure_file`` at ``scipy/meson.build``). It is the authority for
  ``scipy.__version__`` and is imported unconditionally by ``scipy/__init__.py``
  (``from scipy.version import version as __version__``).
- ``scipy/__config__.py`` -- configured by meson from ``scipy/__config__.py.in``
  (``configure_file`` at ``scipy/meson.build``, near line 782). ``scipy/__init__``
  imports ``from scipy.__config__ import show as show_config`` and *fails closed*
  (raises ``ImportError`` with the "you cannot import SciPy while being in the
  SciPy source directory" message) when that import fails.

The pact witness stages SciPy from source-derived module roots (the sealed
extension roots under ``tmp/pact_scipy_*`` and the off-the-shelf checkout under
``bench/friends/repos/scipy_off_the_shelf``). Because those roots carry SciPy's
*source* tree, the two build-generated modules are absent, and the source-derived
support-file closure silently drops them. At witness runtime
``from scipy.version import version`` (once ``__config__`` is provided) raises
``ImportError``.

This tool is the class-level generator: it re-derives both modules from SciPy's
**own** authoritative producers and materializes them into every witness SciPy
module root.

- ``version.py`` is produced by executing SciPy's committed ``tools/gitversion.py
  --write`` in the checkout. The version string is sourced from SciPy's
  ``pyproject.toml`` and the git revision from the checkout HEAD -- never a
  hand-typed literal.
- ``__config__.py`` is copied from the real meson-generated build output when
  present (the fully substituted module a SciPy wasm meson build emits). No
  fabricated compiler / BLAS / LAPACK / pybind11 metadata is synthesized; if no
  authoritative build output is found the tool **fails closed** and names both
  the expected build-output path and the meson step that must produce it.

Usage::

    python tools/pact_witness_scipy_generated_modules.py [--check] [--repo-root DIR]
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


# SciPy source checkout, relative to the repo root.
_SCIPY_SOURCE_REL = "bench/friends/repos/scipy_off_the_shelf"

# SciPy's own version generator lives at the repo root of the checkout
# (``tools/gitversion.py``), NOT inside the ``scipy`` package (this is where it
# differs from NumPy, whose generator is ``numpy/_build_utils/gitversion.py``).
_SCIPY_GITVERSION_REL = "tools/gitversion.py"

# Witness SciPy package roots that must carry the generated modules. Each entry
# is a directory that (when it exists) contains a ``scipy/`` package the witness
# stages/resolves from. Mirrors the scipy roots selected by
# tools/proof_queue.py::_pact_witness_native_roots.
_WITNESS_SCIPY_PACKAGE_PARENTS = (
    "tmp/pact_scipy_ndimage_sealed_for_witness_next",
    "tmp/pact_scipy_ndimage_sealed_for_witness",
    "tmp/pact_scipy_ni_label_molt_ext_wasm_cpython_abi",
    "tmp/pact_scipy_ndimage_molt_ext_wasm_cpython_abi",
    _SCIPY_SOURCE_REL,
)

# Candidate locations of a real meson-generated ``scipy/__config__.py``. A full
# SciPy wasm ``meson setup`` runs ``configure_file`` on ``scipy/__config__.py.in``
# and emits the substituted module under ``<build>/scipy/__config__.py``.
_CONFIG_BUILD_CANDIDATES = (
    "tmp/pact_scipy_ndimage_meson_wasm_build_generated/scipy/__config__.py",
    "tmp/pact_scipy_ndimage_meson_wasm_build/scipy/__config__.py",
)

_GENERATED_MODULES = ("version.py", "__config__.py")


def _scipy_pyproject_version(scipy_source_root: Path) -> str:
    """Authoritative SciPy version string, read from SciPy's pyproject.toml."""
    pyproject = scipy_source_root / "pyproject.toml"
    try:
        lines = pyproject.read_text(encoding="utf-8").splitlines()
    except OSError as exc:
        raise GeneratedModuleError(
            f"cannot read SciPy pyproject.toml at {pyproject}: {exc}"
        ) from exc
    for line in lines:
        if line.startswith("version ="):
            return line.split("=", 1)[1].strip().strip("\"'")
    raise GeneratedModuleError(
        f"no 'version =' line in SciPy pyproject.toml at {pyproject}"
    )


def _generate_version_module(scipy_source_root: Path) -> str:
    """Run SciPy's own gitversion.py generator and return version.py content."""
    gitversion = scipy_source_root / _SCIPY_GITVERSION_REL
    if not gitversion.is_file():
        raise GeneratedModuleError(
            f"SciPy gitversion generator not found at {gitversion}; "
            "cannot derive scipy/version.py from SciPy's own authority"
        )
    with tempfile.TemporaryDirectory() as tmpdir:
        out_path = Path(tmpdir) / "version.py"
        proc = subprocess.run(
            [sys.executable, str(gitversion), "--write", str(out_path)],
            capture_output=True,
            text=True,
            cwd=str(scipy_source_root),
        )
        if proc.returncode != 0:
            raise GeneratedModuleError(
                f"SciPy gitversion.py failed (rc={proc.returncode}): "
                f"{proc.stderr.strip() or proc.stdout.strip()}"
            )
        try:
            content = out_path.read_text(encoding="utf-8")
        except OSError as exc:
            raise GeneratedModuleError(
                f"SciPy gitversion.py did not write {out_path}: {exc}"
            ) from exc
    expected_version = _scipy_pyproject_version(scipy_source_root)
    if f'version = "{expected_version}"' not in content:
        raise GeneratedModuleError(
            "generated scipy/version.py does not carry the pyproject version "
            f"{expected_version!r}; refusing to materialize an inconsistent "
            "version module"
        )
    return content


def _generate_config_module(repo_root: Path) -> tuple[str, Path]:
    """Locate the real meson-generated scipy/__config__.py and return it."""
    for rel in _CONFIG_BUILD_CANDIDATES:
        candidate = repo_root / rel
        if candidate.is_file():
            content = candidate.read_text(encoding="utf-8")
            if "@C_COMP@" in content or "@CYTHON_COMP@" in content:
                raise GeneratedModuleError(
                    f"{candidate} still has unsubstituted meson @VARS@; it is a "
                    "template, not a generated build output"
                )
            return content, candidate
    raise GeneratedModuleError(
        "no authoritative meson-generated scipy/__config__.py found; searched: "
        + ", ".join(str(repo_root / rel) for rel in _CONFIG_BUILD_CANDIDATES)
        + ". SciPy's __config__.py is emitted by meson's configure_file on "
        "scipy/__config__.py.in (scipy/meson.build ~L782) during a full "
        "`meson setup` of the SciPy wasm cross build (compiler ids/versions + "
        "BLAS/LAPACK/pybind11 dependency resolution, e.g. -Dblas=none "
        "-Dlapack=none). Run that setup so scipy/__config__.py exists, then "
        "re-run; refusing to fabricate build/compiler metadata."
    )


def _witness_scipy_package_dirs(repo_root: Path) -> list[Path]:
    """Existing ``scipy/`` package dirs across the witness module roots."""
    dirs: list[Path] = []
    for parent_rel in _WITNESS_SCIPY_PACKAGE_PARENTS:
        scipy_dir = repo_root / parent_rel / "scipy"
        if scipy_dir.is_dir():
            dirs.append(scipy_dir)
    return dirs


def _version_source(repo_root: Path) -> str:
    scipy_source_root = repo_root / _SCIPY_SOURCE_REL
    if not scipy_source_root.is_dir():
        raise GeneratedModuleError(
            f"SciPy source checkout not found at {scipy_source_root}; provision "
            "the off-the-shelf SciPy checkout before materializing generated "
            "modules"
        )
    return _generate_version_module(scipy_source_root)


def materialize_version(repo_root: Path) -> list[Path]:
    """Write scipy/version.py into every witness SciPy package dir.

    version.py is fully derivable from SciPy's own authorities, so it is always
    materialized independently of __config__.py (the deeper meson-setup blocker).
    """
    version_src = _version_source(repo_root)
    scipy_dirs = _witness_scipy_package_dirs(repo_root)
    if not scipy_dirs:
        raise GeneratedModuleError(
            "no witness SciPy package roots present on disk; nothing to "
            "materialize. Stage the pact witness roots first."
        )
    written: list[Path] = []
    for scipy_dir in scipy_dirs:
        target = scipy_dir / "version.py"
        if not target.is_file() or target.read_text(encoding="utf-8") != version_src:
            target.write_text(version_src, encoding="utf-8")
        written.append(target)
    return written


def materialize_config(repo_root: Path) -> list[Path]:
    """Write scipy/__config__.py into every witness SciPy package dir.

    Fails closed if no authoritative meson-generated __config__.py exists.
    """
    config_src, _origin = _generate_config_module(repo_root)
    scipy_dirs = _witness_scipy_package_dirs(repo_root)
    if not scipy_dirs:
        raise GeneratedModuleError(
            "no witness SciPy package roots present on disk; nothing to "
            "materialize. Stage the pact witness roots first."
        )
    written: list[Path] = []
    for scipy_dir in scipy_dirs:
        target = scipy_dir / "__config__.py"
        if not target.is_file() or target.read_text(encoding="utf-8") != config_src:
            target.write_text(config_src, encoding="utf-8")
        written.append(target)
    return written


def materialize(repo_root: Path) -> list[Path]:
    """Write both generated modules into every witness SciPy package dir.

    Raises GeneratedModuleError (from materialize_config) if __config__.py has no
    authoritative source. version.py is materialized first so the real increment
    persists even when __config__.py is still blocked.
    """
    written = materialize_version(repo_root)
    written.extend(materialize_config(repo_root))
    return written


def check(repo_root: Path) -> list[str]:
    """Return a list of problems; empty means every root is complete + current."""
    problems: list[str] = []
    scipy_dirs = _witness_scipy_package_dirs(repo_root)
    if not scipy_dirs:
        return ["no witness SciPy package roots present on disk"]

    # version.py -- derivable authority.
    try:
        version_src = _version_source(repo_root)
    except GeneratedModuleError as exc:
        problems.append(str(exc))
        version_src = None
    if version_src is not None:
        for scipy_dir in scipy_dirs:
            target = scipy_dir / "version.py"
            if not target.is_file():
                problems.append(f"missing generated module: {target}")
            elif target.read_text(encoding="utf-8") != version_src:
                problems.append(
                    f"stale generated module (does not match SciPy's authority): "
                    f"{target}"
                )

    # __config__.py -- meson-setup blocker until a real build output exists.
    try:
        config_src, _origin = _generate_config_module(repo_root)
    except GeneratedModuleError as exc:
        problems.append(str(exc))
        config_src = None
    if config_src is not None:
        for scipy_dir in scipy_dirs:
            target = scipy_dir / "__config__.py"
            if not target.is_file():
                problems.append(f"missing generated module: {target}")
            elif target.read_text(encoding="utf-8") != config_src:
                problems.append(
                    f"stale generated module (does not match SciPy's authority): "
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
                print("STALE scipy generated modules:")
                for problem in problems:
                    print(f"  {problem}")
                return 1
            print("OK scipy build-generated modules present + current")
            return 0
        # version.py always materializes; __config__.py may be blocked (fail
        # closed) -- surface both outcomes honestly.
        written = materialize_version(repo_root)
        for path in written:
            print(f"materialized {path}")
        config_written = materialize_config(repo_root)
        for path in config_written:
            print(f"materialized {path}")
        return 0
    except GeneratedModuleError as exc:
        print(f"FAIL {exc}", file=sys.stderr)
        return 2


if __name__ == "__main__":
    raise SystemExit(main())
