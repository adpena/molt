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

    def manifest_payload(self) -> dict[str, Any]:
        return {
            "pyx": str(self.pyx_path),
            "original_c": str(self.original_c),
            "regenerated_c": str(self.regenerated_c),
            "standalone": True,
            "cython_version": self.cython_version,
            "cython_argv": list(self.cython_argv),
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

# A ``cimport X`` / ``from X cimport ...`` statement. Group 1 = the ``from`` module,
# group 2 = the bare ``cimport`` target. Leading-dot (relative) forms are handled
# by the caller (they need no external ``-I``).
_CIMPORT_RE = re.compile(
    r"^[ \t]*(?:from[ \t]+([.\w]+)[ \t]+cimport\b|cimport[ \t]+([.\w]+))",
    re.MULTILINE,
)


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
            text = src.read_text(encoding="utf-8", errors="ignore")
        except OSError:
            continue
        for match in _CIMPORT_RE.finditer(text):
            module = match.group(1) or match.group(2) or ""
            if not module or module.startswith("."):
                continue
            top = module.split(".", 1)[0]
            if top and top not in _CYTHON_BUILTIN_CIMPORT_ROOTS:
                names.add(top)
    return tuple(sorted(names))


def _cimport_pxd_roots(interpreter: str, packages: Sequence[str]) -> tuple[Path, ...]:
    """Install-parent dirs of ``packages`` that ship a top-level ``__init__.pxd``.

    Cython resolves ``cimport numpy`` by searching the ``-I`` dirs for
    ``numpy/__init__.pxd`` — which ships INSIDE the installed numpy package
    (``<site-packages>/numpy/__init__.pxd``), NOT in ``numpy.get_include()``
    (that dir holds only the C headers). Standalone ``python -m cython`` with only
    the C-header dir on ``-I`` therefore fails to resolve ``cimport numpy``:
    ``np.uintp_t`` binds to ``None`` and Cython 3.1+ crashes in
    ``MethodDispatcherTransform`` on the first typed-memoryview subscript with a
    variable index (e.g. scipy ``_ni_label.pyx``). Pinning an older Cython only
    MASKS the crash (it emits different, untrustworthy C) — a fail-open, not a fix.
    The structural fix: for each source-declared cimport dependency, put its
    install PARENT on ``-I`` so ``<pkg>/__init__.pxd`` resolves against the exact
    package the build interpreter will import. Fail-safe: packages that are not
    installed, or ship no top-level ``.pxd``, are skipped — never breaking a build
    for a pure-C or bundled-namespace cimport.
    """
    if not packages:
        return ()
    roots: list[Path] = []
    seen: set[Path] = set()
    for pkg in packages:
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
        if not parent:
            continue
        resolved = Path(parent).resolve()
        if resolved not in seen and resolved.is_dir():
            seen.add(resolved)
            roots.append(resolved)
    return tuple(roots)


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
    resolved_includes = _cython_include_dirs(
        pyx_path=pyx_path,
        plan_include_dirs=include_dirs,
        cimport_pxd_roots=_cimport_pxd_roots(
            interpreter, _parse_cimported_packages(pyx_path)
        ),
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
