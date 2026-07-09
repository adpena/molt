#!/usr/bin/env python3
"""Stage SciPy's full pure-Python subtree into the pact witness sealed roots.

Root cause this closes (E1 witness / scipy-custody closure completeness -- the
SciPy analogue of ``pact_witness_numpy_python_closure``):

The pact witness admits SciPy as an *external static package*: the sealed roots
under ``tmp/pact_scipy_*`` own SciPy's compiled ndimage extensions
(``_nd_image.molt.wasm`` / ``_ni_label.molt.wasm``) + their manifests, and the
build resolves SciPy's pure-Python support surface as the import-driven closure
of those sealed roots. The sealed roots were staged with only the C-ext-adjacent
files (``scipy/__init__.py`` + ``scipy/ndimage/__init__.py``), so every
pure-Python submodule ``scipy/__init__.py`` imports -- ``_distributor_init``,
``_external.packaging_version.version``, ``_lib._ccallback``, ``_lib._testutils``,
``version``, ``__config__``, and their transitive closure -- lives only in the
off-the-shelf source tree, so the closure BFS cannot resolve them and each one
fail-closes at witness runtime with ``ModuleNotFoundError: No module named
'scipy.<sub>'``.

This tool is the systematic, one-pass fix: it mirrors SciPy's entire importable
pure-Python subtree from the off-the-shelf source checkout into every witness
sealed root, so the *existing* import-driven support-file closure can resolve and
stage SciPy's full transitive Python import closure. It does NOT hand-pick
submodules and it does NOT loosen any import; it provides the modules SciPy's
``__init__`` genuinely imports.

Unlike NumPy, SciPy's *installed* layout differs from its *source* layout for one
subpackage: ``scipy/_external/packaging_version`` installs its modules from a
``src/`` subdirectory (``py3.install_sources(files('src/version.py',
'src/_structures.py'), subdir: 'scipy/_external/packaging_version')`` in
``scipy/_external/packaging_version/meson.build``). ``scipy/__init__`` imports
``from scipy._external.packaging_version.version import Version, parse``, so the
mirror must place those two files at the *installed* path (dropping ``src/``),
not the source path. That relocation is applied from the meson ``install_sources``
authority (parsed, not hand-typed).

It also materializes SciPy's build-generated modules (``version.py`` always;
``__config__.py`` when an authoritative meson build output exists) via
``pact_witness_scipy_generated_modules``. The compiled ndimage artifacts already
staged (``*.molt.wasm`` + ``*.extension_manifest.json``) and the package-root
generated modules are never overwritten by the source mirror.

Usage::

    python tools/pact_witness_scipy_python_closure.py [--check] [--repo-root DIR]
"""

from __future__ import annotations

import argparse
import re
import sys
from pathlib import Path

if str(Path(__file__).resolve().parent) not in sys.path:
    sys.path.insert(0, str(Path(__file__).resolve().parent))
import pact_witness_scipy_generated_modules as _generated  # noqa: E402


class ClosureStagingError(Exception):
    """A fail-closed error while staging SciPy's pure-Python closure."""


def _repo_root() -> Path:
    return Path(__file__).resolve().parent.parent


# SciPy source checkout, relative to the repo root (the pure-Python authority).
_SCIPY_SOURCE_REL = _generated._SCIPY_SOURCE_REL

# Witness sealed roots that must carry SciPy's full pure-Python subtree. The
# off-the-shelf checkout is the SOURCE, not a target.
_WITNESS_SEALED_PARENTS = tuple(
    parent
    for parent in _generated._WITNESS_SCIPY_PACKAGE_PARENTS
    if parent != _SCIPY_SOURCE_REL
)

# Directory segments never imported at witness runtime; excluded to keep the
# sealed tree to SciPy's importable surface. ``tests`` packages are unit-test
# trees that ``scipy/__init__`` never imports.
_EXCLUDED_DIR_SEGMENTS = frozenset({"tests"})

# Package-root files the source mirror must never overwrite: the build-generated
# modules owned by pact_witness_scipy_generated_modules. Guarded by *relative
# path* (not basename) so nested modules that share the name -- e.g.
# ``_external/packaging_version/version.py`` -- are still mirrored.
_MIRROR_PROTECTED_RELPATHS = frozenset({"version.py", "__config__.py"})

# meson.build files that relocate installed sources out of a ``src/`` subdir. The
# relocation is derived from the meson ``install_sources`` authority below.
_INSTALL_RENAME_MESON = "scipy/_external/packaging_version/meson.build"


def _scipy_source_root(repo_root: Path) -> Path:
    root = repo_root / _SCIPY_SOURCE_REL / "scipy"
    if not root.is_dir():
        raise ClosureStagingError(
            f"SciPy source checkout not found at {root}; provision the "
            "off-the-shelf SciPy checkout before staging the witness closure"
        )
    return root


def _install_renames(repo_root: Path) -> dict[str, str]:
    """Map source-relative subdir -> installed-relative subdir for renamed sources.

    Derived from SciPy's own meson ``install_sources`` declaration so the mirror
    tracks the authoritative installed layout rather than hard-coding names.
    """
    renames: dict[str, str] = {}
    meson = repo_root / _SCIPY_SOURCE_REL / _INSTALL_RENAME_MESON
    if not meson.is_file():
        # The subpackage is absent from this checkout; nothing to relocate.
        return renames
    text = meson.read_text(encoding="utf-8")
    subdir_match = re.search(r"subdir:\s*'scipy/([^']+)'", text)
    if subdir_match is None:
        raise ClosureStagingError(
            f"could not find install subdir in {meson}; SciPy install layout "
            "may have changed -- refusing to guess the packaging_version path"
        )
    install_subdir = subdir_match.group(1)  # e.g. _external/packaging_version
    for src in re.findall(r"'([^']+\.py)'", text):
        # src like 'src/version.py' -> source subdir 'src'
        src_path = Path(src)
        if src_path.parent == Path("."):
            continue
        source_subdir = (Path("_external/packaging_version") / src_path.parent).as_posix()
        renames[source_subdir] = install_subdir
    return renames


def _installed_relpath(source_rel: Path, renames: dict[str, str]) -> Path:
    """Translate a source-relative .py path to its installed-relative path."""
    posix = source_rel.as_posix()
    for source_subdir, install_subdir in renames.items():
        prefix = source_subdir + "/"
        if posix.startswith(prefix):
            return Path(install_subdir) / posix[len(prefix):]
    return source_rel


def _importable_python_sources(scipy_source: Path) -> list[Path]:
    """Every importable ``*.py`` under SciPy's source tree, tests excluded."""
    sources: list[Path] = []
    for path in scipy_source.rglob("*.py"):
        rel_parts = path.relative_to(scipy_source).parts
        if any(part in _EXCLUDED_DIR_SEGMENTS for part in rel_parts[:-1]):
            continue
        sources.append(path)
    return sources


def _mirror_plan(scipy_source: Path, renames: dict[str, str]) -> list[tuple[Path, Path]]:
    """List of (source_path, installed_relpath) pairs, protected names excluded."""
    plan: list[tuple[Path, Path]] = []
    for source_path in _importable_python_sources(scipy_source):
        source_rel = source_path.relative_to(scipy_source)
        installed_rel = _installed_relpath(source_rel, renames)
        if installed_rel.as_posix() in _MIRROR_PROTECTED_RELPATHS:
            continue
        plan.append((source_path, installed_rel))
    return plan


def _sealed_scipy_dirs(repo_root: Path) -> list[Path]:
    dirs: list[Path] = []
    for parent in _WITNESS_SEALED_PARENTS:
        scipy_dir = repo_root / parent / "scipy"
        if scipy_dir.is_dir():
            dirs.append(scipy_dir)
    return dirs


def _mirror_into(
    plan: list[tuple[Path, Path]], sealed_scipy: Path
) -> list[Path]:
    written: list[Path] = []
    for source_path, installed_rel in plan:
        target = sealed_scipy / installed_rel
        content = source_path.read_bytes()
        if not target.is_file() or target.read_bytes() != content:
            target.parent.mkdir(parents=True, exist_ok=True)
            target.write_bytes(content)
        written.append(target)
    return written


def stage(repo_root: Path) -> list[Path]:
    """Mirror SciPy's pure-Python subtree + generated modules into every root.

    The pure-Python mirror and version.py always persist. __config__.py is
    materialized only when a real meson build output exists; otherwise this
    raises ClosureStagingError naming that precise remaining blocker -- but only
    after the pure-Python + version.py increment has been written to disk.
    """
    scipy_source = _scipy_source_root(repo_root)
    renames = _install_renames(repo_root)
    sealed_dirs = _sealed_scipy_dirs(repo_root)
    if not sealed_dirs:
        raise ClosureStagingError(
            "no witness sealed SciPy roots present on disk; stage the pact "
            "witness roots first."
        )
    plan = _mirror_plan(scipy_source, renames)
    written: list[Path] = []
    for sealed_scipy in sealed_dirs:
        written.extend(_mirror_into(plan, sealed_scipy))
    # version.py is fully derivable and always materialized (real increment).
    written.extend(_generated.materialize_version(repo_root))
    # __config__.py fails closed here if no authoritative meson output exists.
    written.extend(_generated.materialize_config(repo_root))
    return written


def check(repo_root: Path) -> list[str]:
    """Return closure-completeness problems; empty means every root is complete."""
    problems: list[str] = []
    try:
        scipy_source = _scipy_source_root(repo_root)
        renames = _install_renames(repo_root)
    except ClosureStagingError as exc:
        return [str(exc)]
    sealed_dirs = _sealed_scipy_dirs(repo_root)
    if not sealed_dirs:
        return ["no witness sealed SciPy roots present on disk"]
    plan = _mirror_plan(scipy_source, renames)
    for sealed_scipy in sealed_dirs:
        for source_path, installed_rel in plan:
            target = sealed_scipy / installed_rel
            if not target.is_file():
                problems.append(f"missing staged module: {target}")
                continue
            if target.read_bytes() != source_path.read_bytes():
                problems.append(f"stale staged module (source drift): {target}")
    problems.extend(_generated.check(repo_root))
    return problems


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument(
        "--check",
        action="store_true",
        help="verify every sealed root carries SciPy's full pure-Python closure",
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
                print(f"STALE scipy witness closure ({len(problems)} problems):")
                for problem in problems[:20]:
                    print(f"  {problem}")
                if len(problems) > 20:
                    print(f"  ... (+{len(problems) - 20} more)")
                return 1
            print("OK scipy pure-Python closure staged + current in every root")
            return 0
        written = stage(repo_root)
        print(f"staged {len(written)} scipy module files across witness roots")
        return 0
    except (ClosureStagingError, _generated.GeneratedModuleError) as exc:
        print(f"FAIL {exc}", file=sys.stderr)
        return 2


if __name__ == "__main__":
    raise SystemExit(main())
