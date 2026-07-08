"""Standalone Cython-extension C regeneration authority (R73.2).

Molt compiles a source-recompiled extension from the C that the upstream build
emitted. For a Cython module that C is *generated* from a ``.pyx`` — and an
upstream build (scipy's meson) runs Cython with ``--shared scipy._cyutility``,
so the emitted C carries a ``__Pyx_modinit_shared_function_import_code`` helper
that imports the shared-utility module ``scipy._cyutility`` at
``Py_mod_exec`` time. Molt has no such module, so consuming that ``--shared``
C fails closed at extension init.

This module is the single authority for *how Molt gets a Cython extension's C*:
instead of consuming the upstream ``--shared`` C, Molt re-runs Cython
**standalone** (``cython -3`` with no ``--shared``) from the package's own
``.pyx`` and ``.pxd`` search paths, so each extension embeds its own utility
code and imports no shared-utility module. This is R73.2's bounded bypass:
one custody path, no host fallback, no fake module — the regenerated C is the
package's own source recompiled.

Cython itself is auto-provisioned (R73.2): the required version is derived from
the package's build metadata (``build-system.requires``) and validated against
the interpreter that will run Cython, installing it into that interpreter's
environment when missing, failing closed with a precise, actionable diagnostic
when it cannot be provisioned.
"""

from __future__ import annotations

import json
import os
import re
import subprocess
import sys
from dataclasses import dataclass
from pathlib import Path
from typing import Any, Mapping, Sequence

# Cython emits the ``.pyx`` -> C generated file with the same stem. A Cython
# source group in a meson build pairs the ``.pyx`` (a non-compiled input) with
# the generated ``.c``/``.cpp`` by stem, so stem matching is the pairing key.
_CYTHON_SOURCE_SUFFIX = ".pyx"
_CYTHON_GENERATED_SUFFIXES = {".c", ".cpp", ".cxx", ".cc"}

# ``build-system.requires`` entries look like ``Cython>=3.0.6`` /
# ``cython==3.1.*``; the requirement string is the version authority.
_CYTHON_REQUIREMENT_RE = re.compile(
    r"^\s*cython\b\s*(?P<spec>.*)$",
    re.IGNORECASE,
)
_VERSION_CLAUSE_RE = re.compile(
    r"(?P<op><=|>=|==|~=|!=|<|>)\s*(?P<version>[0-9][0-9A-Za-z_.*+-]*)"
)


@dataclass(frozen=True)
class _CythonRequirement:
    """A parsed ``build-system.requires`` Cython constraint."""

    raw: str
    clauses: tuple[tuple[str, str], ...]

    def minimum_major(self) -> int | None:
        best: int | None = None
        for op, version in self.clauses:
            if op not in {">=", "==", "~=", ">"}:
                continue
            major = _leading_int(version)
            if major is None:
                continue
            if best is None or major > best:
                best = major
        return best


@dataclass(frozen=True)
class CythonRegeneration:
    """Result of regenerating one Cython extension's C standalone."""

    pyx_path: Path
    original_c: Path
    regenerated_c: Path
    cython_version: str
    cython_argv: tuple[str, ...]
    cimport_packages: tuple[str, ...] = ()
    cimport_pxd_roots: tuple[Path, ...] = ()
    cimport_header_include_dirs: tuple[Path, ...] = ()

    def manifest_payload(self) -> dict[str, Any]:
        return {
            "pyx": str(self.pyx_path),
            "original_c": str(self.original_c),
            "regenerated_c": str(self.regenerated_c),
            "standalone": True,
            "cython_version": self.cython_version,
            "cython_argv": list(self.cython_argv),
            "cimport_packages": list(self.cimport_packages),
            "cimport_pxd_roots": [str(path) for path in self.cimport_pxd_roots],
            "cimport_header_include_dirs": [
                str(path) for path in self.cimport_header_include_dirs
            ],
        }


def _leading_int(version: str) -> int | None:
    match = re.match(r"(\d+)", version)
    return int(match.group(1)) if match else None


def parse_cython_build_requirement(
    build_system_requires: Sequence[Any] | None,
) -> _CythonRequirement | None:
    """Return the Cython constraint from ``build-system.requires`` if present."""
    if not build_system_requires:
        return None
    for raw_entry in build_system_requires:
        if not isinstance(raw_entry, str):
            continue
        # Strip environment markers (``; python_version < "3.13"``) and extras.
        requirement = raw_entry.split(";", 1)[0].strip()
        requirement = requirement.split("[", 1)[0].strip()
        match = _CYTHON_REQUIREMENT_RE.match(requirement)
        if match is None:
            continue
        spec = match.group("spec")
        clauses = tuple(
            (clause.group("op"), clause.group("version"))
            for clause in _VERSION_CLAUSE_RE.finditer(spec)
        )
        return _CythonRequirement(raw=raw_entry.strip(), clauses=clauses)
    return None


def cython_build_requirement_from_pyproject(
    pyproject: Mapping[str, Any] | None,
) -> _CythonRequirement | None:
    if not isinstance(pyproject, Mapping):
        return None
    build_system = pyproject.get("build-system")
    if not isinstance(build_system, Mapping):
        return None
    requires = build_system.get("requires")
    if not isinstance(requires, Sequence):
        return None
    return parse_cython_build_requirement(requires)


def _installed_cython_version(python_exe: str) -> str | None:
    try:
        result = subprocess.run(
            [
                python_exe,
                "-c",
                "import Cython; print(Cython.__version__)",
            ],
            capture_output=True,
            text=True,
            timeout=60,
            check=False,
        )
    except (OSError, subprocess.SubprocessError):
        return None
    if result.returncode != 0:
        return None
    version = result.stdout.strip().splitlines()
    return version[0].strip() if version else None


def _pip_specifier(requirement: _CythonRequirement | None) -> str:
    if requirement is None:
        return "Cython"
    if requirement.clauses:
        spec = ",".join(f"{op}{version}" for op, version in requirement.clauses)
        return f"Cython{spec}"
    # A bare ``Cython`` requirement with no version clause: pin the major the
    # build metadata implies (scipy needs Cython 3.x); default to >=3.0.
    return "Cython>=3.0"


def provision_cython(
    *,
    python_exe: str,
    requirement: _CythonRequirement | None,
) -> tuple[str | None, str | None]:
    """Ensure Cython is importable by ``python_exe``; return (version, error).

    Fail-closed: if Cython is absent and cannot be pip-installed into the
    interpreter's environment, return a precise, actionable diagnostic.
    """
    version = _installed_cython_version(python_exe)
    required_major = requirement.minimum_major() if requirement is not None else None
    if version is not None:
        installed_major = _leading_int(version)
        if (
            required_major is not None
            and installed_major is not None
            and installed_major < required_major
        ):
            # Upgrade to satisfy the build's stated floor.
            version = None
        else:
            return version, None

    specifier = _pip_specifier(requirement)
    try:
        install = subprocess.run(
            [
                python_exe,
                "-m",
                "pip",
                "install",
                "--disable-pip-version-check",
                specifier,
            ],
            capture_output=True,
            text=True,
            timeout=900,
            check=False,
        )
    except (OSError, subprocess.SubprocessError) as exc:
        return None, (
            "Molt could not auto-provision Cython for the source-recompiled "
            f"extension build. Install {specifier!r} into {python_exe!r}: {exc}"
        )
    if install.returncode != 0:
        detail = (install.stderr or install.stdout or "").strip().splitlines()
        tail = detail[-1] if detail else f"exit code {install.returncode}"
        return None, (
            "Molt could not auto-provision Cython for the source-recompiled "
            f"extension build. `pip install {specifier}` into {python_exe!r} "
            f"failed: {tail}. Install a compatible Cython manually, or make the "
            "interpreter's environment writable."
        )
    version = _installed_cython_version(python_exe)
    if version is None:
        return None, (
            "Molt auto-provisioned Cython but it is still not importable by "
            f"{python_exe!r}. Install {specifier!r} manually."
        )
    if required_major is not None and requirement is not None:
        installed_major = _leading_int(version)
        if installed_major is not None and installed_major < required_major:
            return None, (
                f"Molt provisioned Cython {version} but the extension build "
                f"metadata requires Cython {requirement.raw!r} "
                f"(major >= {required_major}). Install a compatible Cython."
            )
    return version, None


# Cython's bundled cimport namespaces (shipped in ``Cython/Includes``) resolve
# without a site-packages ``-I``; they must never be treated as installed-package
# cimport dependencies.
_CYTHON_BUILTIN_CIMPORT_ROOTS = frozenset(
    {"cpython", "libc", "libcpp", "cython", "posix", "openmp"}
)

# ``cimport X`` may name a comma-separated module list with aliases. ``from X
# cimport ...`` contributes the ``X`` side. Leading-dot modules are relative and
# need no external package-root include.
_BARE_CIMPORT_RE = re.compile(r"^[ \t]*cimport[ \t]+(?P<modules>.+?)\s*$")
_FROM_CIMPORT_RE = re.compile(
    r"^[ \t]*from[ \t]+(?P<module>[.\w]+)[ \t]+cimport\b"
)
_CIMPORT_AS_RE = re.compile(r"\s+as\s+[A-Za-z_][A-Za-z0-9_]*\s*$")
_TOP_LEVEL_CIMPORT_RE = re.compile(r"^(?P<top>[A-Za-z_][A-Za-z0-9_]*)")


def _parse_cimported_packages(pyx_path: Path) -> tuple[str, ...]:
    """Top-level installed packages a ``.pyx`` (and its sibling ``.pxd``) ``cimport``s.

    Cython resolves ``cimport numpy`` by finding ``numpy/__init__.pxd`` on a ``-I``
    dir. The set of packages that must be resolvable is DERIVED FROM THE SOURCE —
    every distinct top-level name in a ``cimport`` — so the include surface is
    correct for ANY cimport dependency, not a hard-coded per-package list. Relative
    cimports (leading ``.``) and Cython's bundled namespaces
    (cpython/libc/libcpp/...) are excluded: they need no site-packages ``-I``.
    """
    names: set[str] = set()
    scan: list[Path] = [pyx_path]
    if pyx_path.parent.is_dir():
        scan.extend(sorted(pyx_path.parent.glob("*.pxd")))
    for src in scan:
        try:
            lines = src.read_text(encoding="utf-8", errors="ignore").splitlines()
        except OSError:
            continue
        for raw_line in lines:
            line = raw_line.split("#", 1)[0].strip()
            if not line:
                continue
            from_match = _FROM_CIMPORT_RE.match(line)
            if from_match is not None:
                _add_cimport_top_level_package(names, from_match.group("module"))
                continue
            cimport_match = _BARE_CIMPORT_RE.match(line)
            if cimport_match is None:
                continue
            for module in cimport_match.group("modules").split(","):
                module = _CIMPORT_AS_RE.sub("", module.strip())
                _add_cimport_top_level_package(names, module)
    return tuple(sorted(names))


def _add_cimport_top_level_package(names: set[str], module: str) -> None:
    module = module.strip()
    if not module or module.startswith("."):
        return
    match = _TOP_LEVEL_CIMPORT_RE.match(module)
    if match is None:
        return
    top = match.group("top")
    if top and top not in _CYTHON_BUILTIN_CIMPORT_ROOTS:
        names.add(top)


def _cimport_pxd_roots(
    interpreter: str,
    packages: Sequence[str],
    search_roots: Sequence[Path] = (),
) -> tuple[Path, ...]:
    """Dirs ``D`` such that ``D/<pkg>/__init__.pxd`` exists, for each cimported ``pkg``.

    Cython resolves ``cimport numpy`` by searching the ``-I`` dirs for
    ``numpy/__init__.pxd`` — which ships INSIDE the numpy *package* dir
    (``numpy/__init__.pxd``), NOT in ``numpy/_core/include`` (that holds only the C
    headers). Standalone ``python -m cython`` with only the C-header dir on ``-I``
    therefore fails to resolve ``cimport numpy``: ``np.uintp_t`` binds to ``None``
    and Cython 3.1+ crashes in ``MethodDispatcherTransform`` on the first
    typed-memoryview subscript with a variable index (e.g. scipy ``_ni_label.pyx``).
    Pinning an older Cython only MASKS the crash (it emits different, untrustworthy
    C) — a fail-open, not a fix.

    A source-recompiled witness builds against the package's OWN source tree, not a
    pip install, so resolution must cover both:

      1. Source tree — the numpy ``.pxd`` lives at ``<numpy-src>/numpy/__init__.pxd``
         while the build plan only puts ``<numpy-src>/numpy/_core/include`` on ``-I``.
         Walk each ``search_root`` (the plan include dirs + the ``.pyx`` package
         roots) up through its ancestors and add the first ancestor ``A`` where
         ``A/<pkg>/__init__.pxd`` exists.
      2. Installed package — fall back to the build interpreter's installed ``pkg``
         (``find_spec``) when it ships a top-level ``__init__.pxd``.

    Fail-safe: packages resolvable by neither route (pure-C or bundled-namespace
    cimports) are skipped — never breaking a build.
    """
    if not packages:
        return ()
    roots: list[Path] = []
    seen: set[Path] = set()

    def _add(path: Path) -> bool:
        resolved = path.resolve()
        if resolved in seen or not resolved.is_dir():
            return False
        seen.add(resolved)
        roots.append(resolved)
        return True

    resolved_search: list[Path] = []
    for root in search_roots:
        try:
            resolved_search.append(Path(root).resolve())
        except OSError:
            continue

    for pkg in packages:
        # (1) Source tree: nearest ancestor of a search root hosting <pkg>/__init__.pxd.
        found = False
        for root in resolved_search:
            for ancestor in (root, *root.parents):
                if (ancestor / pkg / "__init__.pxd").is_file():
                    _add(ancestor)
                    found = True
                    break
            if found:
                break
        if found:
            continue
        # (2) Installed package in the build interpreter.
        probe = (
            "import importlib.util, os\n"
            f"spec = importlib.util.find_spec({pkg!r})\n"
            "locs = getattr(spec, 'submodule_search_locations', None) if spec else None\n"
            "pkg_dir = (list(locs)[0] if locs else None)\n"
            "print(os.path.dirname(pkg_dir) if pkg_dir and "
            "os.path.exists(os.path.join(pkg_dir, '__init__.pxd')) else '')\n"
        )
        try:
            completed = subprocess.run(
                [interpreter, "-c", probe],
                capture_output=True,
                text=True,
                timeout=60,
            )
        except (OSError, subprocess.SubprocessError):
            continue
        parent = completed.stdout.strip()
        if parent:
            _add(Path(parent))
    return tuple(roots)


def _cimport_package_roots_from_env() -> tuple[Path, ...]:
    """Package/source custody roots supplied through ``MOLT_MODULE_ROOTS``.

    ``MOLT_MODULE_ROOTS`` is the existing import-custody surface for external
    package roots. Entries may be plain paths or dotted aliases
    (``pkg.name=/path/to/pkg``); Cython regeneration only needs the path side.
    """
    raw_roots = os.environ.get("MOLT_MODULE_ROOTS", "")
    if not raw_roots:
        return ()
    roots: list[Path] = []
    seen: set[Path] = set()
    for entry in raw_roots.split(os.pathsep):
        if not entry:
            continue
        _prefix, sep, value = entry.partition("=")
        raw_path = value if sep else entry
        if not raw_path.strip():
            continue
        try:
            path = Path(raw_path.strip()).expanduser().resolve()
        except OSError:
            continue
        if path in seen or not path.is_dir():
            continue
        seen.add(path)
        roots.append(path)
    return tuple(roots)


def _cimport_header_dirs_from_pxd_roots(
    packages: Sequence[str],
    pxd_roots: Sequence[Path],
) -> tuple[Path, ...]:
    """Package-owned C-header include dirs adjacent to resolved ``.pxd`` roots.

    Cython ``.pxd`` files can generate C includes such as ``numpy/arrayobject.h``.
    For source-recompiled packages, those headers live in the same package source
    custody root that supplied ``pkg/__init__.pxd``. Resolve that shape
    generically: any directory named ``pkg`` under the package source tree that
    contains C headers contributes its parent as an include root.
    """
    requested = tuple(
        sorted(
            {
                package
                for package in packages
                if package and package not in _CYTHON_BUILTIN_CIMPORT_ROOTS
            }
        )
    )
    if not requested or not pxd_roots:
        return ()
    roots: list[Path] = []
    seen: set[Path] = set()

    def _add(path: Path) -> None:
        try:
            resolved = path.resolve()
        except OSError:
            return
        if resolved in seen or not resolved.is_dir():
            return
        seen.add(resolved)
        roots.append(resolved)

    def _has_header_child(path: Path) -> bool:
        try:
            return any(
                child.is_file() and child.suffix.lower() in {".h", ".hh", ".hpp", ".hxx"}
                for child in path.iterdir()
            )
        except OSError:
            return False

    for root in pxd_roots:
        try:
            resolved_root = Path(root).resolve()
        except OSError:
            continue
        for package in requested:
            package_dir = resolved_root / package
            if not package_dir.is_dir():
                continue
            if _has_header_child(package_dir):
                _add(package_dir.parent)
            try:
                package_named_dirs = (
                    candidate
                    for candidate in package_dir.rglob(package)
                    if candidate.is_dir()
                )
                for candidate in package_named_dirs:
                    if _has_header_child(candidate):
                        _add(candidate.parent)
            except OSError:
                continue
    return tuple(roots)


def _cimport_header_include_dirs(
    interpreter: str,
    packages: Sequence[str],
    *,
    cimport_pxd_roots: Sequence[Path] = (),
) -> tuple[Path, ...]:
    """Package-owned C header include dirs exposed by cimported dependencies.

    A pxd-shipping dependency may emit C that must be compiled against headers
    from the same package version that supplied the pxd. Resolve generic
    source-custody header roots first, then ``get_include()`` hooks in the build
    interpreter; packages without either are skipped fail-safe, leaving
    source-plan metadata authoritative.
    """
    requested = tuple(
        sorted(
            {
                package
                for package in packages
                if package and package not in _CYTHON_BUILTIN_CIMPORT_ROOTS
            }
        )
    )
    if not requested:
        return ()
    roots: list[Path] = []
    seen: set[Path] = set()

    def _add(path: Path) -> None:
        try:
            resolved = path.resolve()
        except OSError:
            return
        if resolved in seen or not resolved.is_dir():
            return
        seen.add(resolved)
        roots.append(resolved)

    for path in _cimport_header_dirs_from_pxd_roots(
        requested,
        cimport_pxd_roots,
    ):
        _add(path)

    probe = r"""
import importlib
import json
import sys
from pathlib import Path

roots = []
for name in json.loads(sys.argv[1]):
    try:
        module = importlib.import_module(name)
        getter = getattr(module, "get_include", None)
        if not callable(getter):
            continue
        value = getter()
    except Exception:
        continue
    values = value if isinstance(value, (list, tuple)) else [value]
    for raw in values:
        try:
            path = Path(raw).resolve()
        except (OSError, TypeError):
            continue
        if path.is_dir():
            roots.append(str(path))
print(json.dumps(roots))
"""
    try:
        result = subprocess.run(
            [interpreter, "-c", probe, json.dumps(requested)],
            capture_output=True,
            text=True,
            timeout=60,
            check=False,
        )
    except (OSError, subprocess.SubprocessError):
        return ()
    if result.returncode != 0:
        return tuple(roots)
    payload = _json_from_probe_stdout(result.stdout)
    if not isinstance(payload, list):
        return tuple(roots)
    for path in _existing_unique_dirs(payload):
        _add(path)
    return tuple(roots)


def _json_from_probe_stdout(stdout: str) -> Any:
    for line in reversed((stdout or "").splitlines()):
        line = line.strip()
        if not line:
            continue
        try:
            return json.loads(line)
        except json.JSONDecodeError:
            continue
    return None


def _existing_unique_dirs(values: Sequence[Any]) -> tuple[Path, ...]:
    dirs: list[Path] = []
    seen: set[Path] = set()
    for value in values:
        if not isinstance(value, str):
            continue
        try:
            path = Path(value).resolve()
        except OSError:
            continue
        if path in seen or not path.is_dir():
            continue
        seen.add(path)
        dirs.append(path)
    return tuple(dirs)


def _cython_include_dirs(
    *,
    pyx_path: Path,
    plan_include_dirs: Sequence[Path],
    cimport_pxd_roots: Sequence[Path] = (),
) -> tuple[Path, ...]:
    """Include (``.pxd``/``.pxi``) search paths for standalone regeneration.

    Cython resolves ``cimport`` / ``include`` against ``-I`` dirs. The search
    surface is the union of: the package's own source tree (the ``.pyx`` directory
    and its package roots), any plan-declared include dirs, and the install parents
    of cimport-able dependencies that ship a ``__init__.pxd`` (``cimport_pxd_roots``
    — derived from the source via :func:`_parse_cimported_packages` and resolved by
    :func:`_cimport_pxd_roots`). numpy's ``.pxd`` does NOT resolve without an
    explicit ``-I`` to its package parent.
    """
    dirs: list[Path] = []
    seen: set[Path] = set()

    def add(path: Path) -> None:
        resolved = path.resolve()
        if resolved in seen or not resolved.is_dir():
            return
        seen.add(resolved)
        dirs.append(resolved)

    add(pyx_path.parent)
    # Walk up to the package root(s): a ``.pyx`` in ``scipy/ndimage/src`` may
    # ``cimport`` siblings via ``scipy/...`` relative includes.
    for parent in pyx_path.parents:
        add(parent)
        if (parent / "pyproject.toml").is_file() or (parent / "setup.py").is_file():
            break
    for include_dir in plan_include_dirs:
        add(Path(include_dir))
    # Source-declared cimport dependencies (numpy/__init__.pxd for `cimport numpy`).
    for pxd_root in cimport_pxd_roots:
        add(Path(pxd_root))
    return tuple(dirs)


def regenerate_cython_c_standalone(
    *,
    pyx_path: Path,
    original_c: Path,
    out_dir: Path,
    include_dirs: Sequence[Path],
    cython_version: str,
    python_exe: str | None = None,
    package_roots: Sequence[Path] = (),
) -> tuple[CythonRegeneration | None, str | None]:
    """Run ``cython -3`` STANDALONE for ``pyx_path`` into ``out_dir``.

    Standalone (no ``--shared``) makes Cython embed its utility code in the
    emitted C, so the module carries no ``scipy._cyutility`` /
    ``__Pyx_modinit_shared_function_import`` shared-utility import.
    """
    interpreter = python_exe or sys.executable
    is_cpp = original_c.suffix.lower() in {".cpp", ".cxx", ".cc"}
    out_dir.mkdir(parents=True, exist_ok=True)
    regenerated_c = out_dir / f"{pyx_path.stem}{original_c.suffix.lower()}"
    cimport_packages = _parse_cimported_packages(pyx_path)
    cimport_search_roots = [
        *include_dirs,
        pyx_path.parent,
        *package_roots,
        *_cimport_package_roots_from_env(),
    ]
    cimport_pxd_roots = _cimport_pxd_roots(
        interpreter,
        cimport_packages,
        search_roots=cimport_search_roots,
    )
    cimport_header_include_dirs = _cimport_header_include_dirs(
        interpreter,
        cimport_packages,
        cimport_pxd_roots=cimport_pxd_roots,
    )
    resolved_includes = _cython_include_dirs(
        pyx_path=pyx_path,
        plan_include_dirs=include_dirs,
        cimport_pxd_roots=cimport_pxd_roots,
    )
    argv: list[str] = [interpreter, "-m", "cython", "-3"]
    if is_cpp:
        argv.append("--cplus")
    for include_dir in resolved_includes:
        argv.extend(["-I", str(include_dir)])
    argv.extend([str(pyx_path), "-o", str(regenerated_c)])
    try:
        result = subprocess.run(
            argv,
            capture_output=True,
            text=True,
            timeout=600,
            check=False,
        )
    except (OSError, subprocess.SubprocessError) as exc:
        return None, (
            f"Molt could not regenerate the Cython extension {pyx_path.name} "
            f"standalone: {exc}"
        )
    if result.returncode != 0 or not regenerated_c.is_file():
        # Preserve the FULL Cython diagnostic (traceback + message), not just the
        # last line: Cython crashes (e.g. the MethodDispatcherTransform failure on
        # an unresolved `cimport numpy`) put the actionable cause in the traceback,
        # and keeping only stderr[-1] hid it as an "unclassified" failure. Cap the
        # captured text so a runaway log can't dominate the diagnostic.
        detail = (result.stderr or result.stdout or "").strip()
        if not detail:
            detail = f"exit code {result.returncode}"
        elif len(detail) > 8000:
            detail = detail[:4000] + "\n...[truncated]...\n" + detail[-4000:]
        return None, (
            f"Standalone `cython -3` regeneration of {pyx_path.name} failed "
            f"(argv: {' '.join(argv)}):\n{detail}"
        )
    return (
        CythonRegeneration(
            pyx_path=pyx_path.resolve(),
            original_c=original_c.resolve(),
            regenerated_c=regenerated_c.resolve(),
            cython_version=cython_version,
            cython_argv=tuple(argv),
            cimport_packages=cimport_packages,
            cimport_pxd_roots=cimport_pxd_roots,
            cimport_header_include_dirs=cimport_header_include_dirs,
        ),
        None,
    )


def pair_generated_c_with_pyx(
    *,
    generated_c: Path,
    pyx_candidates: Sequence[Path],
) -> Path | None:
    """Return the ``.pyx`` that produced ``generated_c`` (matched by stem)."""
    if generated_c.suffix.lower() not in _CYTHON_GENERATED_SUFFIXES:
        return None
    stem = generated_c.stem
    for candidate in pyx_candidates:
        if candidate.suffix.lower() == _CYTHON_SOURCE_SUFFIX and candidate.stem == stem:
            return candidate.resolve()
    return None
