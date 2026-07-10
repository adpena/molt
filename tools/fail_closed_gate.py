#!/usr/bin/env python3
"""Enforcement gate: make the "hack / workaround / ecosystem-baked / fail-open"
poison class un-landable.

A *poison site* is a place where Molt abandons its own contract — one authority
per invariant, source-recompiled ecosystem support, and fail-CLOSED on unsupported
behavior — in favour of a shortcut that silently ships a wrong or degraded answer:

  * ecosystem_baked   — a package-owned header/source (numpy / scipy / pandas
    symbols) vendored *and defined* inside a Molt-owned tree instead of flowing
    through source-recompiled extension custody. A baked ``PyArray_*`` struct or
    ``static inline`` body is upstream semantics frozen into Molt; it rots the
    moment upstream moves and it is not the reusable primitive the contract wants.

  * ecosystem_build_crutch — a package-specific source-plan/build helper or
    generated-header/config authoring path that hand-maintains what the
    package's own build metadata and build system must own. Molt may run and
    introspect upstream Meson/setup.py/Cython/etc.; it must not clone that logic
    into one-off ``regen_scipy_*`` / ``regen_numpy_*`` lanes.

  * ecosystem_reimplementation — a Molt-owned Python package root for NumPy,
    SciPy, pandas, tinygrad, or a similar third-party library. Molt must compile
    upstream package Python plus source-recompiled extensions through package
    custody and auto-provision the required build backend/toolchain/libraries;
    it must not grow a shadow ``src/molt/stdlib/<package>`` implementation.

  * research-quarantine breach -- a retained package-semantics clone is allowed
    only as quarantined research/reference material under an explicit marker. It
    must never be added to ``PYTHONPATH``, ``MOLT_MODULE_ROOTS``, module roots,
    import resolution, build inputs, or shipped runtime/package paths. Tests may
    load it only through the approved explicit fixture loader.

  * fail_open_stub    — a ``#[no_mangle] extern "C"`` ABI export that returns a
    plausible NON-NULL / success sentinel on a path it does not actually
    implement (``MoltObject::none()`` / ``PyList_New(0)`` placeholder,
    ``wrapping_*`` on an arbitrary-precision int, ``let _ = <arg>;`` that drops a
    real argument, a ``_ => success`` catch-all), or a ``-> c_int`` export that
    returns a mismatch code with no ``PyErr`` set. The C extension calling it
    cannot tell success from a lie — the opposite of failing closed.

  * fail_open_backend_dispatch — a backend op-emitter dispatch catch-all
    (``_ =>`` / ``other =>`` / an ``emit_unsupported*`` / ``emit_op_other``
    helper) that, for an op kind the backend does NOT lower, emits a FABRICATED
    result value (``local x = nil`` / ``MoltValue::None`` / a placeholder) and
    lets compilation continue. The generated program then runs with a silent
    wrong value where the unlowered op's result should be — the codegen analogue
    of ``fail_open_stub``. The end-state backend records the unsupported op into
    a fail-closed accumulator that ``compile_checked`` rejects; a catch-all that
    emits a placeholder is the poison. (Backends whose catch-all ``panic!``s /
    ``bail!``s, or whose catch-all is a REAL generic lowering — e.g. MLIR's
    ``emit_opaque_molt_op`` that emits a properly-named opaque op — are fail-CLOSED
    and are NOT this class.)

  * duplicate_authority — two live copies of the SAME authority (e.g. two
    ``Python.h`` files, two copies of a CPython header) that must be kept in sync
    by hand. One of them is always about to drift.

  * todo_as_plan      — a ``HACK`` / ``WORKAROUND`` / ``for now`` / ``status:``
    marker reachable from a *shipped* support surface (a public stdlib API or an
    exported ABI symbol that advertises the capability). A stub module that
    honestly says ``status:planned`` and does NOT claim support is fine; a shipped
    API whose body says "ignore for now" is a poison site.

``tools/fail_closed_registry.toml`` is the single authority: one row per KNOWN
poison site, classified into the poison classes above, each naming the
``resolution_row`` (the rip task driving it to zero). This gate is the teeth. It
is modelled EXACTLY on ``tools/degrade_to_slow_gate.py`` + its registry ratchet:

  * DISCOVERY SCAN  — content scans (ecosystem-baked, ecosystem-build-crutch,
    ecosystem-reimplementation, fail-open ABI/backend dispatch, todo-as-plan,
    duplicate authority) sweep the real trees and emit the live poison-site set.
    A curated allowlist keeps sound conservatism, thin compat forwarders,
    package-build-system custody, the
    package-custody CLASSIFIER, and honest ``status:planned`` stubs OUT.

  * DRIFT RECONCILIATION — every discovered site MUST have a registry row. A new,
    unregistered poison site FAILS the gate, exactly like a new op-kind missing
    from ``op_kinds.toml``.

  * RATCHET — the per-class count of registered sites is monotonically
    non-increasing against the per-class baseline. The poison classes can only
    shrink; the rip arcs lower the baselines as they delete sites.

  * TEETH — every row must name a ``resolution_row``; anchor signatures must still
    exist verbatim (a site cannot silently move out from under its row).

Run standalone::

    uv run --active --project . --python 3.12 python tools/fail_closed_gate.py

Exit 0 = green, 1 = drift / ratchet / anchor failure (violations printed).
"""

from __future__ import annotations

import fnmatch
import os
import re
import subprocess
import sys
import tomllib
from dataclasses import dataclass, field
from pathlib import Path
from typing import Iterable

REPO_ROOT = Path(__file__).resolve().parents[1]
REGISTRY_PATH = REPO_ROOT / "tools" / "fail_closed_registry.toml"

# ─────────────────────────────────────────────────────────────────────────────
# Pruned directory walk (DX: keep the scan fast on the shared build volume)
# ─────────────────────────────────────────────────────────────────────────────
#
# The scans below sweep the Molt SOURCE trees (include/, src/molt, runtime/…).
# On the shared checkout / D: build volume those trees accumulate build output,
# byte-compiled caches, and VCS metadata (``__pycache__`` full of ``.pyc``,
# ``target/``, ``.git/`` …). A naive ``Path.rglob("*")`` enumerates and ``stat``s
# every one of those entries even though the per-file suffix filter later drops
# them — which is what turned this gate into a ~2-minute landing bottleneck.
#
# ``_walk_files`` is a pruned ``os.walk`` that skips those build/cache/VCS dirs
# IN PLACE (editing ``dirnames``) so they are never descended into. This changes
# NO detection semantics: the pruned directory names below hold only ``.pyc`` /
# build output / VCS metadata, never ``.h``/``.c``/``.py``/``.rs`` source under
# audit. To make that a PROVABLE invariant rather than an assumption, the walk is
# suffix-aware and FAILS CLOSED: before it prunes a directory it verifies (via a
# recursive scan that stops at the first hit) that the subtree holds NO
# audited-suffix file. If one is found, the directory is NOT pruned — it is
# walked exactly like the old ``rglob`` — and a loud warning is logged. A file is
# therefore never silently skipped; pruning only ever elides ``.pyc`` / build /
# VCS entries that the per-file suffix filter would have dropped anyway.
#
# Exact-name prunes plus a glob prune for build-output ``*_metadata`` dirs.
_PRUNE_DIR_NAMES = frozenset(
    {
        ".git",
        "target",
        ".venv",
        "node_modules",
        "build",
        "dist",
        "__pycache__",
        "tmp",
        ".molt_cache",
        "worktrees",
    }
)
_PRUNE_DIR_GLOBS = ("*_metadata",)


def _prune_candidate(name: str) -> bool:
    if name in _PRUNE_DIR_NAMES:
        return True
    return any(fnmatch.fnmatch(name, pat) for pat in _PRUNE_DIR_GLOBS)


def _subtree_has_audited_source(base: Path, suffixes: frozenset[str]) -> bool:
    """True iff ``base`` (recursively) contains a file whose suffix is audited.
    Stops at the first hit. Only called for prune-candidate dirs, which in a
    healthy tree hold only ``.pyc`` / build output — so this returns ``False``
    immediately after scanning cheap non-source entries."""

    stack = [base]
    while stack:
        cur = stack.pop()
        try:
            with os.scandir(cur) as it:
                for entry in it:
                    try:
                        if entry.is_dir(follow_symlinks=False):
                            stack.append(Path(entry.path))
                        elif os.path.splitext(entry.name)[1] in suffixes:
                            return True
                    except OSError:
                        continue
        except OSError:
            continue
    return False


def _walk_files(base: Path, suffixes: frozenset[str]) -> Iterable[Path]:
    """Yield every file under ``base`` in sorted order, pruning build/cache/VCS
    directories that cannot contain audited source. Provably equivalent to
    ``sorted(base.rglob("*"))`` filtered to files: a prune-candidate directory is
    pruned ONLY after ``_subtree_has_audited_source`` confirms it holds no
    ``suffixes`` file; otherwise it is walked normally (and a loud warning is
    emitted) so no audited file is ever silently skipped."""

    for dirpath, dirnames, filenames in os.walk(base):
        # Prune in place so os.walk never descends into excluded trees. A
        # candidate is only actually pruned once proven source-free; sort the
        # survivors for deterministic traversal order.
        kept: list[str] = []
        for d in dirnames:
            if _prune_candidate(d):
                sub = Path(dirpath) / d
                if _subtree_has_audited_source(sub, suffixes):
                    print(
                        f"fail-closed gate: WARNING not pruning {sub.as_posix()!r}: "
                        f"it holds audited source ({sorted(suffixes)}); scanning it "
                        "in full to preserve detection coverage.",
                        file=sys.stderr,
                    )
                    kept.append(d)
                # else: proven source-free — safe to prune.
            else:
                kept.append(d)
        dirnames[:] = sorted(kept)
        d = Path(dirpath)
        for fname in sorted(filenames):
            yield d / fname


_VALID_CLASSES = frozenset(
    {
        "ecosystem_baked",
        "ecosystem_build_crutch",
        "ecosystem_reimplementation",
        "fail_open_stub",
        "duplicate_authority",
        "todo_as_plan",
        "fail_open_backend_dispatch",
    }
)

# ─────────────────────────────────────────────────────────────────────────────
# Scan A — ecosystem-baked package-owned symbol definitions
# ─────────────────────────────────────────────────────────────────────────────

# Trees a Molt-owned baked package header/source could hide in.
_ECOSYSTEM_SCAN_DIRS = (
    "include",
    "src/molt",
)
# Rust/native source + include trees are also swept for baked package impls.
_ECOSYSTEM_SCAN_DIRS_RUST = ("runtime",)

# Package-owned symbol families whose *definition* (not mere mention) inside a
# Molt-owned tree is the ecosystem-baked poison.
_PACKAGE_TOKEN_RE = re.compile(r"(?:numpy|NumPy|NPY_|PyArray_|scipy|pandas)")

# A *definition* of a package-owned symbol: a typedef/struct/enum/union that
# names one, a `static inline` body, a `} PyArray*;` struct close, or a function
# body `PyArray_xxx(...) {`. This is what separates baked semantics from a thin
# `#include <numpy/...>` forwarder or a comment.
_DEFINITION_RES = (
    re.compile(r"^\s*typedef\s+(?:struct|union|enum)\b.*(?:NPY_|PyArray_|npy_)"),
    re.compile(
        r"^\s*(?:typedef\s+)?(?:struct|union|enum)\s+(?:NPY_|PyArray_|_?Npy|npy_)"
    ),
    re.compile(r"^\s*\}\s*(?:PyArray_|PyUFunc_|Npy|npy_)[A-Za-z0-9_]*\s*;"),
    re.compile(r"^\s*static\s+(?:NPY_INLINE\s+|inline\s+)"),
    re.compile(r"^\s*#define\s+NPY_SIZEOF_"),
    re.compile(r"\bPyArray_[A-Za-z0-9_]+\s*\([^;]*\)\s*\{"),
)

# Allowlisted files: the package-custody CLASSIFIER (correctly ROUTES these
# symbols to source-recompiled custody and fails closed — the right primitive),
# and any file that is a thin `#include <numpy/...>` compat forwarder.
_ECOSYSTEM_ALLOWLIST = frozenset(
    {
        "src/molt/c_api_symbols.py",
        "src/molt/cli/external_native.py",
    }
)

_FORWARDER_INCLUDE_RE = re.compile(r'#\s*include(?:_next)?\s*[<"](?:numpy/|scipy/)')


def _is_thin_forwarder(text: str) -> bool:
    """A header that only re-includes an upstream package header (and guards) —
    it bakes no symbols of its own, so it is NOT ecosystem-baked poison."""

    has_include = bool(_FORWARDER_INCLUDE_RE.search(text))
    if not has_include:
        return False
    # If it ALSO carries a real package definition it is not a thin forwarder.
    for line in text.splitlines():
        for rex in _DEFINITION_RES:
            if rex.search(line):
                return False
    return True


def _strip_block_comments(text: str) -> str:
    return re.sub(r"/\*.*?\*/", "", text, flags=re.DOTALL)


def _defines_package_symbol(text: str) -> bool:
    stripped = _strip_block_comments(text)
    for raw_line in stripped.splitlines():
        line = raw_line.rstrip("\n")
        # Ignore line comments.
        no_line_comment = re.sub(r"//.*$", "", line)
        if not _PACKAGE_TOKEN_RE.search(no_line_comment):
            continue
        for rex in _DEFINITION_RES:
            if rex.search(no_line_comment):
                return True
    return False


@dataclass(frozen=True)
class DiscoveredSite:
    scan: str  # "A" | "B" | "C"
    cls: str
    file: str
    signature: str
    line: int
    detail: str


def _iter_files(
    root: Path, dirs: Iterable[str], suffixes: tuple[str, ...]
) -> Iterable[Path]:
    suffix_set = frozenset(suffixes)
    for rel in dirs:
        base = root / rel
        if not base.exists():
            continue
        for path in _walk_files(base, suffix_set):
            if not path.is_file():
                continue
            if path.suffix not in suffixes:
                continue
            posix = path.as_posix()
            if "/target/" in posix or "/tests/" in posix or "/test/" in posix:
                continue
            yield path


def discover_ecosystem_baked(root: Path) -> list[DiscoveredSite]:
    found: list[DiscoveredSite] = []
    header_dirs = _ECOSYSTEM_SCAN_DIRS
    for path in _iter_files(root, header_dirs, (".h", ".c", ".py")):
        rel = path.relative_to(root).as_posix()
        if rel in _ECOSYSTEM_ALLOWLIST:
            continue
        try:
            text = path.read_text(encoding="utf-8", errors="replace")
        except OSError:
            continue
        if _is_thin_forwarder(text):
            continue
        if path.suffix == ".py":
            # For Python surfaces, only the numpyconfig-overlay authoring block
            # (which BAKES an upstream _numpyconfig.h fixup) counts — plain
            # mentions of NPY_ constants in string literals do not define symbols.
            if "#include_next" in text and "_numpyconfig.h" in text:
                found.append(
                    DiscoveredSite(
                        "A",
                        "ecosystem_baked",
                        rel,
                        '#include_next "_numpyconfig.h"',
                        _first_line(text, '#include_next "_numpyconfig.h"'),
                        "bakes an upstream numpy _numpyconfig.h fixup overlay",
                    )
                )
            continue
        if _defines_package_symbol(text):
            sig = _first_definition_signature(text)
            found.append(
                DiscoveredSite(
                    "A",
                    "ecosystem_baked",
                    rel,
                    sig,
                    _first_line(text, sig),
                    "defines package-owned (numpy/scipy/pandas) symbols in-tree",
                )
            )
    return found


def _first_definition_signature(text: str) -> str:
    stripped = _strip_block_comments(text)
    for raw_line in stripped.splitlines():
        no_line_comment = re.sub(r"//.*$", "", raw_line)
        if not _PACKAGE_TOKEN_RE.search(no_line_comment):
            continue
        for rex in _DEFINITION_RES:
            if rex.search(no_line_comment):
                return no_line_comment.strip()
    return ""


def _first_line(text: str, needle: str) -> int:
    for i, line in enumerate(text.splitlines(), start=1):
        if needle in line:
            return i
    return 0


# ─────────────────────────────────────────────────────────────────────────────
# Scan F — ecosystem build/source-plan crutches
# ─────────────────────────────────────────────────────────────────────────────

# A package build/source-plan crutch is not a package-owned C symbol definition;
# it is Molt tooling cloning one package's build knowledge by hand. The correct
# primitive is to run/provision/introspect the package's own build system
# (pyproject/Meson/setup.py/Cython/etc.) through a generic custody path.
_BUILD_CRUTCH_SCAN_DIRS = (
    "tools",
    "src/molt/cli",
)
_ECOSYSTEM_PACKAGE_NAMES = ("numpy", "scipy", "pandas", "tinygrad")
_PACKAGE_REGEN_HELPER_RE = re.compile(
    r"(?:^|/)regen_(?:numpy|scipy|pandas|tinygrad)[A-Za-z0-9_]*\.py$"
)
_PACKAGE_CONFIG_WRITE_RE = re.compile(
    r"(?:"
    r"scipy_config\.h"
    r"|_numpyconfig\.h"
    r"|numpyconfig\.h"
    r"|(?:numpy|scipy|pandas|tinygrad)[A-Za-z0-9_./\\-]*config\.h"
    r"|config[A-Za-z0-9_./\\-]*(?:numpy|scipy|pandas|tinygrad)[A-Za-z0-9_./\\-]*\.h"
    r")",
    re.IGNORECASE,
)
_PACKAGE_OVERLAY_AUTHORITY_RE = re.compile(
    r"(?:"
    r"source[_-]?plan"
    r"|source_plan_target_facts"
    r"|generated[_-]?header"
    r"|header[_-]?overlay"
    r"|config[_-]?overlay"
    r"|include[_-]?overlay"
    r"|overlay[_-]?(?:dir|root|path)"
    r")",
    re.IGNORECASE,
)
_PACKAGE_NAME_RE = re.compile(
    r"(?:^|[^A-Za-z0-9])(?:numpy|scipy|pandas|tinygrad)(?:$|[^A-Za-z0-9])",
    re.IGNORECASE,
)
_BUILD_CRUTCH_ALLOWLIST = frozenset(
    {
        # The gate names the poison class it enforces; that prose is not a
        # package build helper.
        "tools/fail_closed_gate.py",
        # Generic source-extension authorities may mention package examples while
        # consuming upstream Meson/compile_commands metadata for any package.
        "src/molt/cli/source_extensions.py",
        "src/molt/cli/source_extension_cython.py",
        "src/molt/cli/source_extension_toolchain.py",
        # Pact-witness pure-Python closure stagers + build-generated-module
        # materializers. These are seal verifiers/adapters (per the docstring
        # rationale below): they mirror the package's OWN upstream pure-Python
        # subtree into the sealed witness roots and materialize version.py/
        # __config__.py from the package's OWN authorities (gitversion, the real
        # meson build output). They author no Molt-owned source-plan/config/
        # header overlay -- their docstrings merely describe the architecture --
        # so they are not the ecosystem_build_crutch poison shape.
        "tools/pact_witness_numpy_python_closure.py",
        "tools/pact_witness_scipy_python_closure.py",
        "tools/pact_witness_scipy_generated_modules.py",
        # Pact-witness numpy _umath_linalg (linalg) WASM producer. It consumes
        # numpy's OWN upstream Meson intro-targets/compile_commands (produced by
        # regen_numpy_multiarray_meson_wasm.py) and invokes the GENERIC
        # `molt extension build` CLI -- it authors no Molt-owned source-plan/
        # config/header overlay. Its docstring merely describes the npymath
        # exclusion, which is a GENERIC, gated `molt extension build
        # --source-plan-exclude-linked-static-library` feature (source_extensions
        # .py, itself allowlisted above), not a package-specific crutch.
        "tools/build_numpy_umath_linalg_wasm.py",
        # Pact-witness scipy _ccallback_c WASM producer. It regenerates the C
        # from scipy's OWN _ccallback_c.pyx via the GENERIC standalone-Cython
        # authority (source_extension_cython.py, allowlisted above), materializes
        # scipy_config.h from scipy's OWN scipy_config.h.in via meson's
        # configure_file rule (package custody, not a Molt-authored overlay --
        # identical to a real scipy meson wasm-cross configure_file output), and
        # invokes the GENERIC `molt extension build` CLI. It authors no Molt-owned
        # source-plan/config/header overlay; the synthesized intro-targets/
        # compile_commands are the generic source-plan shape the CLI consumes.
        "tools/build_scipy_ccallback_c_wasm.py",
    }
)


def discover_ecosystem_build_crutches(root: Path) -> list[DiscoveredSite]:
    found: list[DiscoveredSite] = []
    for path in _iter_files(root, _BUILD_CRUTCH_SCAN_DIRS, (".py",)):
        rel = path.relative_to(root).as_posix()
        if rel in _BUILD_CRUTCH_ALLOWLIST:
            continue
        text = path.read_text(encoding="utf-8", errors="replace")
        lower = text.lower()
        if _PACKAGE_REGEN_HELPER_RE.search(rel):
            if any(
                token in lower
                for token in (
                    "source_plan",
                    "source plan",
                    "meson",
                    "compile_commands",
                    "ninja",
                    "wasi",
                )
            ):
                signature = path.name
                found.append(
                    DiscoveredSite(
                        "F",
                        "ecosystem_build_crutch",
                        rel,
                        signature,
                        1,
                        "package-specific regen/build helper; use a generic "
                        "upstream-build-system custody path instead",
                    )
                )
                continue
        config_match = _PACKAGE_CONFIG_WRITE_RE.search(text)
        if config_match and (
            "write_text" in text
            or "source_plan_target_facts" in text
            or "generated-header" in lower
        ):
            line_no = _first_line(text, config_match.group(0))
            signature = _line_at(text, line_no).strip()
            found.append(
                DiscoveredSite(
                    "F",
                    "ecosystem_build_crutch",
                    rel,
                    signature,
                    line_no,
                    "Molt-authored package config/generated-header path; derive "
                    "this from the package build system instead",
                )
            )
            continue
        overlay = _package_specific_overlay_signature(rel, text)
        if overlay is not None:
            signature, line_no = overlay
            found.append(
                DiscoveredSite(
                    "F",
                    "ecosystem_build_crutch",
                    rel,
                    signature,
                    line_no,
                    "package-specific source-plan/config/header overlay; route "
                    "through generic upstream build metadata custody instead",
                )
            )
    return found


def _package_specific_overlay_signature(
    rel: str, text: str
) -> tuple[str, int] | None:
    """Return the first line that authors a protected-package build overlay.

    Package-specific tooling names alone are not poison: adapters, probes, and
    seal verifiers can legitimately name NumPy/SciPy/etc. The poison shape is a
    non-generic source-plan/config/header overlay authority for one protected
    package.
    """

    rel_stem = Path(rel).stem.replace("-", "_")
    rel_has_package = bool(_PACKAGE_NAME_RE.search(rel_stem))
    rel_has_overlay = bool(_PACKAGE_OVERLAY_AUTHORITY_RE.search(rel_stem))
    for line_no, raw_line in enumerate(text.splitlines(), start=1):
        line = raw_line.strip()
        if not line or line.startswith("#"):
            continue
        line_has_package = bool(_PACKAGE_NAME_RE.search(line))
        line_has_overlay = bool(_PACKAGE_OVERLAY_AUTHORITY_RE.search(line))
        if (line_has_package and line_has_overlay) or (
            rel_has_package and rel_has_overlay and line_has_overlay
        ):
            return line, line_no
    return None


def _line_at(text: str, line_no: int) -> str:
    if line_no <= 0:
        return ""
    lines = text.splitlines()
    if line_no > len(lines):
        return ""
    return lines[line_no - 1]


_ECOSYSTEM_REIMPLEMENTATION_ROOTS = ("src/molt/stdlib",)

_QUARANTINED_REFERENCE_ROOTS = {
    "demos/tinygrad/reference_stdlib": "tinygrad",
}
_RESEARCH_QUARANTINE_MARKER = ".molt-research-quarantine"
_QUARANTINE_SCAN_DIRS = (
    "src",
    "runtime",
    "tools",
    "tests",
    "bench",
    "demos",
)
_QUARANTINE_SCAN_SUFFIXES = frozenset(
    {
        ".py",
        ".rs",
        ".toml",
        ".json",
        ".md",
        ".txt",
        ".ps1",
        ".sh",
        ".js",
        ".ts",
        ".yml",
        ".yaml",
    }
)
_QUARANTINED_REFERENCE_TOKENS = (
    "demos/tinygrad/reference_stdlib",
    r"demos\tinygrad\reference_stdlib",
    "reference_stdlib/tinygrad",
    r"reference_stdlib\tinygrad",
    "TINYGRAD_STDLIB",
    "tinygrad_stdlib_context",
    "tinygrad_stdlib_loader",
    "reference_stdlib",
)
_QUARANTINED_TEST_LOADER_TOKENS = frozenset(
    {
        "TINYGRAD_STDLIB",
        "tinygrad_stdlib_context",
        "tinygrad_stdlib_loader",
        "tests.helpers.tinygrad_stdlib_loader",
    }
)
_QUARANTINED_REFERENCE_USE_RE = re.compile(
    r"(?:"
    r"demos[\\/]+tinygrad[\\/]+reference_stdlib"
    r"|reference_stdlib[\\/]+tinygrad"
    r"|tests\.helpers\.tinygrad_stdlib_loader"
    r"|\bTINYGRAD_STDLIB\b"
    r"|\btinygrad_stdlib_context\b"
    r"|\btinygrad_stdlib_loader\b"
    r"|reference_stdlib"
    r")"
)
_QUARANTINE_TEXT_ALLOWLIST = frozenset(
    {
        "tools/fail_closed_gate.py",
        "tests/cli/test_fail_closed_gate.py",
        "tests/helpers/tinygrad_stdlib_loader.py",
        "demos/tinygrad/README.md",
    }
)


def _is_under_rel(rel: str, base_rel: str) -> bool:
    return rel == base_rel or rel.startswith(f"{base_rel}/")


def _iter_quarantine_text_files(root: Path) -> Iterable[Path]:
    git_candidates = _git_grep_quarantine_text_files(root)
    if git_candidates is not None:
        for path in git_candidates:
            yield path
        return

    for base_rel in _QUARANTINE_SCAN_DIRS:
        base = root / base_rel
        if not base.exists():
            continue
        for path in _walk_files(base, _QUARANTINE_SCAN_SUFFIXES):
            if path.is_file() and path.suffix in _QUARANTINE_SCAN_SUFFIXES:
                yield path


def _allowed_quarantined_reference_use(rel: str, token: str) -> bool:
    """Tests may use the sanctioned loader; production paths may not."""

    return rel.startswith("tests/") and token in _QUARANTINED_TEST_LOADER_TOKENS


def _git_grep_quarantine_text_files(root: Path) -> list[Path] | None:
    """Return tracked files containing quarantine tokens, or None outside git.

    The quarantine rule only needs files that mention a small set of path/root
    tokens. In the real checkout, ask git's index-aware grep for that candidate
    set instead of reading every source-ish file. Tmp-root negative controls are
    not git repositories, so they fall back to the deterministic filesystem walk.
    """

    if not (root / ".git").exists():
        return None
    cmd = ["git", "-C", str(root), "grep", "-Il"]
    for token in _QUARANTINED_REFERENCE_TOKENS:
        cmd.extend(["-e", token])
    cmd.append("--")
    cmd.extend(_QUARANTINE_SCAN_DIRS)
    result = subprocess.run(cmd, capture_output=True, text=True)
    if result.returncode == 1:
        return []
    if result.returncode != 0:
        return None
    paths: list[Path] = []
    seen: set[str] = set()
    for raw in result.stdout.splitlines():
        rel = raw.strip().replace("\\", "/")
        if not rel or rel in seen:
            continue
        seen.add(rel)
        path = root / rel
        if path.suffix in _QUARANTINE_SCAN_SUFFIXES:
            paths.append(path)
    return paths


def _ecosystem_reimplementation_signature(text: str, package: str) -> tuple[str, int]:
    for i, raw_line in enumerate(text.splitlines(), start=1):
        line = raw_line.strip()
        if not line:
            continue
        if line.startswith(("from ", "import ", "class ", "def ")) and package in line:
            return line, i
    for i, raw_line in enumerate(text.splitlines(), start=1):
        line = raw_line.strip()
        if not line or line in {'"""', "'''"}:
            continue
        return line, i
    return package, 1


def _ecosystem_reimplementation_site(
    root: Path, path: Path, package: str
) -> DiscoveredSite:
    try:
        text = path.read_text(encoding="utf-8", errors="replace")
    except OSError:
        text = ""
    signature, line = _ecosystem_reimplementation_signature(text, package)
    return DiscoveredSite(
        "A3",
        "ecosystem_reimplementation",
        path.relative_to(root).as_posix(),
        signature,
        line,
        f"Molt-owned Python package root for third-party package {package!r}; "
        "compile upstream package sources/extensions through package custody "
        "instead of reimplementing the library in Molt",
    )


def discover_ecosystem_reimplementations(root: Path) -> list[DiscoveredSite]:
    found: list[DiscoveredSite] = []
    for base_rel in _ECOSYSTEM_REIMPLEMENTATION_ROOTS:
        base = root / base_rel
        if not base.exists():
            continue
        for package in sorted(_ECOSYSTEM_PACKAGE_NAMES):
            module_file = base / f"{package}.py"
            if module_file.exists():
                found.append(
                    _ecosystem_reimplementation_site(root, module_file, package)
                )
            package_dir = base / package
            if not package_dir.is_dir():
                continue
            init_file = package_dir / "__init__.py"
            if init_file.exists():
                found.append(_ecosystem_reimplementation_site(root, init_file, package))
                continue
            for py_file in sorted(package_dir.rglob("*.py")):
                found.append(_ecosystem_reimplementation_site(root, py_file, package))
                break
    return found


# ─────────────────────────────────────────────────────────────────────────────
# Scan B — fail-open ABI exports
# ─────────────────────────────────────────────────────────────────────────────

_ABI_SCAN_DIR = "runtime/molt-cpython-abi/src/api"

# A fail-open marker inside a #[no_mangle] extern "C" body. Each is a way an ABI
# export ships a plausible answer on a path it does not implement.
_FAIL_OPEN_MARKERS = (
    # arbitrary-precision int silently truncated to machine width
    ("wrapping_add", r"\.wrapping_add\("),
    ("wrapping_sub", r"\.wrapping_sub\("),
    ("wrapping_mul", r"\.wrapping_mul\("),
    ("wrapping_pow", r"\.wrapping_pow\("),
    ("wrapping_shl", r"\.wrapping_shl\("),
    ("wrapping_shr", r"\.wrapping_shr\("),
    ("wrapping_abs", r"\.wrapping_abs\("),
    # a real argument dropped on the floor
    ("dropped_arg", r"^\s*let\s+_\s*=\s*[A-Za-z_][A-Za-z0-9_]*\s*;"),
    # a placeholder success value returned from an unimplemented export
    ("none_placeholder", r"MoltObject::none\(\)"),
    ("empty_list_placeholder", r"PyList_New\(0\)"),
)

# Refcount saturation and mask arithmetic are LEGITIMATE wrapping uses (not int
# semantics). Anchor them out by symbol so they are not swept in as poison.
_ABI_ALLOWLIST_SYMBOLS = frozenset(
    {
        "Py_INCREF",
        "Py_DECREF",
        "_Py_INCREF",
        "_Py_DECREF",
        "Py_IncRef",
        "Py_DecRef",
        # PyList_New pre-sizes the runtime list to `size` None slots — the
        # CPython contract itself (Objects/listobject.c calloc's `size` NULL
        # slots + Py_SET_SIZE so indexed PyList_SetItem/SET_ITEM can fill any
        # slot, incl. out of order). MoltObject::none() here is the faithful
        # implementation, not a placeholder success (teeth:
        # tests/test_list_setitem.rs::setitem_places_items_at_index_out_of_order).
        "PyList_New",
    }
)
# Files whose wrapping_* is mask/refcount arithmetic, not int-semantics poison.
_ABI_ALLOWLIST_FILES = frozenset(
    {
        "runtime/molt-cpython-abi/src/api/refcount.rs",
        # numbers.rs uses (value ^ mask).wrapping_sub(mask) for sign extension —
        # a bit-twiddle, not an arbitrary-precision int result.
        "runtime/molt-cpython-abi/src/api/numbers.rs",
    }
)

_NO_MANGLE_RE = re.compile(r"#\[(?:unsafe\()?no_mangle")
_FN_RE = re.compile(
    r'pub\s+(?:unsafe\s+)?extern\s+"C"\s+fn\s+([A-Za-z_][A-Za-z0-9_]*)\s*'
)


@dataclass
class AbiFn:
    name: str
    start: int  # 1-based line of the fn signature
    end: int  # 1-based inclusive last line of the body
    body: str


def _extract_abi_fns(text: str) -> list[AbiFn]:
    """Every `#[no_mangle] ... extern "C" fn NAME` and its brace-balanced body."""

    lines = text.splitlines()
    fns: list[AbiFn] = []
    n = len(lines)
    i = 0
    while i < n:
        if _NO_MANGLE_RE.search(lines[i]):
            # find the fn signature within the next few lines
            j = i
            name = None
            while j < min(i + 6, n):
                m = _FN_RE.search(lines[j])
                if m:
                    name = m.group(1)
                    break
                j += 1
            if name is not None:
                # find opening brace, then brace-balance to the matching close
                k = j
                while k < n and "{" not in lines[k]:
                    k += 1
                if k < n:
                    depth = 0
                    start_body = k
                    end = k
                    for idx in range(k, n):
                        depth += lines[idx].count("{") - lines[idx].count("}")
                        if depth <= 0:
                            end = idx
                            break
                    body = "\n".join(lines[start_body : end + 1])
                    fns.append(AbiFn(name=name, start=j + 1, end=end + 1, body=body))
                    i = end + 1
                    continue
        i += 1
    return fns


def discover_fail_open_abi(root: Path) -> list[DiscoveredSite]:
    base = root / _ABI_SCAN_DIR
    if not base.exists():
        return []
    found: list[DiscoveredSite] = []
    for path in _walk_files(base, frozenset({".rs"})):
        if path.suffix != ".rs":
            continue
        posix = path.as_posix()
        if "/tests/" in posix:
            continue
        rel = path.relative_to(root).as_posix()
        if rel in _ABI_ALLOWLIST_FILES:
            continue
        text = path.read_text(encoding="utf-8", errors="replace")
        for fn in _extract_abi_fns(text):
            if fn.name in _ABI_ALLOWLIST_SYMBOLS:
                continue
            for marker, pat in _FAIL_OPEN_MARKERS:
                rex = re.compile(pat, re.MULTILINE)
                m = rex.search(fn.body)
                if not m:
                    continue
                # Extract the matched line as the anchor signature.
                matched_line = _line_of_offset(fn.body, m.start())
                found.append(
                    DiscoveredSite(
                        "B",
                        "fail_open_stub",
                        rel,
                        matched_line.strip(),
                        fn.start + _line_index_of_offset(fn.body, m.start()),
                        f"{fn.name}: fail-open marker {marker!r}",
                    )
                )
    return found


def _line_of_offset(text: str, offset: int) -> str:
    start = text.rfind("\n", 0, offset) + 1
    end = text.find("\n", offset)
    if end == -1:
        end = len(text)
    return text[start:end]


def _line_index_of_offset(text: str, offset: int) -> int:
    return text.count("\n", 0, offset)


# ─────────────────────────────────────────────────────────────────────────────
# Scan E — fail-open backend op-dispatch catch-all
# ─────────────────────────────────────────────────────────────────────────────
#
# A backend op-emitter whose dispatch catch-all, for an op kind the backend does
# NOT lower, emits a FABRICATED result value and continues — the codegen analogue
# of the fail_open_stub ABI class. The generated program then carries a silent
# wrong value (a Luau ``nil`` / a Rust ``MoltValue::None``) where the unlowered
# op's result should be. The end-state backend instead records the unsupported op
# into a fail-closed accumulator that ``compile_checked`` rejects.
#
# Only the *preview/transpiler* backends that emit source text (luau, rust) can
# express this shape: they have a single catch-all that fabricates a value. The
# native and wasm backends ``panic!`` on an unhandled result-producing op (loud,
# fail-closed) and the MLIR backend's catch-all is a REAL generic lowering
# (``emit_opaque_molt_op`` emits a properly-named opaque op, not a placeholder),
# so those are not swept.
#
# The op-emitter files a fail-open catch-all could hide in. Scoped tightly to the
# dispatch/emit surface so unrelated ``MoltValue::None`` (e.g. a legitimate Python
# ``None`` fill element in ``list_fill_new``) is not swept in.
_BACKEND_DISPATCH_FILES = (
    "runtime/molt-backend-luau/src/luau/op_emitter.rs",
    "runtime/molt-backend-rust/src/rust/op_emitter.rs",
    "runtime/molt-backend-rust/src/rust/op_emitter/gaps.rs",
    "runtime/molt-backend-wasm/src/wasm/op_loop/control_ops.rs",
    "runtime/molt-backend-mlir/src/tir_to_mlir/ops.rs",
    "runtime/molt-backend-mlir/src/tir_to_mlir/opaque_ops.rs",
)

# The fabricated-result signatures. Each is a way a catch-all hands back a
# plausible value for an op it did not lower:
#   * luau: ``local <x> = nil -- [unsupported op: ...]`` — a nil result tagged
#     with the unsupported-op marker.
#   * rust: ``/* <MOLT_STUB marker> */ MoltValue::None`` — a None result wrapped
#     in the stub marker the catch-all emits.
# Both are anchored to the co-located ``unsupported``/``MOLT_STUB`` marker so a
# legitimate ``nil`` default or a real Python-``None`` fill value (which carry no
# such marker) is NOT flagged.
_BACKEND_FAIL_OPEN_RES = (
    (
        "luau_nil_unsupported",
        re.compile(r"=\s*nil\s*--\s*\[unsupported op:"),
    ),
    (
        "rust_none_stub",
        re.compile(r"/\*\s*\{?marker\}?\s*\*/\s*MoltValue::None"),
    ),
    # Generic structural backstop for a NEW backend adding the same shape: a
    # placeholder value (nil / None / none()) emitted next to an
    # ``unsupported``/``unhandled`` op marker on the same line.
    (
        "generic_placeholder_unsupported",
        re.compile(
            r"(?:unsupported|unhandled).{0,80}?(?:=\s*nil\b|MoltValue::None|"
            r"MoltObject::none\(\))",
            re.IGNORECASE,
        ),
    ),
)


def discover_fail_open_backend_dispatch(root: Path) -> list[DiscoveredSite]:
    """Sweep the backend op-emitter dispatch files for a catch-all that fabricates
    a result value for an unlowered op instead of failing closed."""

    found: list[DiscoveredSite] = []
    seen: set[tuple[str, int]] = set()
    for rel in _BACKEND_DISPATCH_FILES:
        path = root / rel
        if not path.exists():
            continue
        text = path.read_text(encoding="utf-8", errors="replace")
        for i, raw_line in enumerate(text.splitlines(), start=1):
            # Ignore lines that are wholly Rust/Lua comments describing the
            # pattern (e.g. the doc-comment in emit_unsupported_op that MENTIONS
            # `MoltValue::None`): a fabricated *emit* is a real statement, not a
            # `//`/`--` comment line.
            stripped = raw_line.strip()
            is_comment = stripped.startswith("//") or stripped.startswith("--")
            for marker, rex in _BACKEND_FAIL_OPEN_RES:
                if not rex.search(raw_line):
                    continue
                # The luau `-- [unsupported op:` and rust `/* {marker} */` forms
                # are real emit statements even though they contain comment
                # syntax; only skip pure leading-comment lines for the GENERIC
                # backstop, which would otherwise match prose.
                if is_comment and marker == "generic_placeholder_unsupported":
                    continue
                key = (rel, i)
                if key in seen:
                    continue
                seen.add(key)
                found.append(
                    DiscoveredSite(
                        "E",
                        "fail_open_backend_dispatch",
                        rel,
                        stripped,
                        i,
                        f"backend op-dispatch catch-all fabricates a result value "
                        f"({marker}) for an unlowered op instead of failing closed",
                    )
                )
                break
    return found


# ─────────────────────────────────────────────────────────────────────────────
# Scan C — todo-as-plan reachable from a shipped support surface
# ─────────────────────────────────────────────────────────────────────────────

_TODO_SCAN_DIRS = (
    "src/molt",
    "runtime/molt-cpython-abi/src",
)

# Markers that a shipped surface is quietly incomplete. Anchored to Molt's
# structured TODO taxonomy (`status:divergent` / `status:partial`) and the
# explicit shortcut markers HACK / WORKAROUND / "ignore for now". `status:planned`
# is EXCLUDED (an honest not-yet-supported declaration). Bare English "for now" is
# NOT a marker on its own: it appears in verbatim upstream CPython comments,
# diagnostic message strings, and fail-closed raises — it only counts when paired
# with an explicit TODO / ignore token on the same line.
_TODO_MARKER_RE = re.compile(
    r"status:(?:divergent|partial)\b|\bHACK\b|\bWORKAROUND\b|"
    r"ignore for now|TODO\b[^\n]*\bfor now\b"
)
_HONEST_PLANNED_RE = re.compile(r"status:planned\b")

# Message-string contexts where a marker token is user-facing text, not a code
# shortcut: a raise/error string or a diagnostic `alternative=`/`message=` kwarg.
_MESSAGE_STRING_RE = re.compile(
    r'raise\s+\w+\(|alternative\s*=|message\s*=|"[^"]*"\s*\)\s*$'
)

# A public def/fn header: a shipped support surface whose BODY the marker sits in.
# `def _x`/`fn _x` (leading underscore) is a private helper — not a shipped surface.
_PUBLIC_DEF_RE = re.compile(
    r"^(\s*)(?:pub\s+)?(?:async\s+)?(?:def|fn)\s+([A-Za-z][A-Za-z0-9_]*)"
)

# Self-negating marker forms: a HACK-marker that describes the gate's OWN
# prevention machinery or a test fixture is not a poison site.
_TODO_ALLOWLIST_SUBSTRINGS = (
    "fail_closed",
    "degrade_to_slow",
    "poison site",
    "synthetic",
)


def _enclosing_public_def(lines: list[str], marker_idx: int) -> str | None:
    """The nearest ``def``/``fn`` whose body encloses ``marker_idx`` — i.e. a def
    header ABOVE the marker at a STRICTLY smaller indent than the marker line, with
    a public (non-underscore) name. Returns the def name, or None if the marker is
    at module level / inside only a private helper. This is what separates a
    per-call silent divergence inside a SHIPPED API from an honest module-level
    scope declaration (which needs no enclosing def and is therefore excluded)."""

    marker_indent = len(lines[marker_idx]) - len(lines[marker_idx].lstrip())
    for j in range(marker_idx - 1, -1, -1):
        cand = lines[j]
        if not cand.strip():
            continue
        indent = len(cand) - len(cand.lstrip())
        m = _PUBLIC_DEF_RE.match(cand)
        if m and indent < marker_indent:
            name = m.group(2)
            if name.startswith("_"):
                # Enclosed only by a private helper — keep scanning outward for a
                # public ancestor at a smaller indent.
                marker_indent = indent
                continue
            return name
        # A non-def line at column 0 above the marker ends the search only if we
        # have descended to module level with no enclosing def found.
        if indent == 0 and not m:
            # Still could be preceded by a public def further up if the marker is
            # itself at indent 0 inside nothing — but indent 0 marker means module
            # level. Stop.
            if marker_indent == 0:
                return None
    return None


def discover_todo_as_plan(root: Path) -> list[DiscoveredSite]:
    found: list[DiscoveredSite] = []
    for path in _iter_files(root, _TODO_SCAN_DIRS, (".py", ".rs")):
        rel = path.relative_to(root).as_posix()
        text = path.read_text(encoding="utf-8", errors="replace")
        lines = text.splitlines()
        for i, line in enumerate(lines):
            if _HONEST_PLANNED_RE.search(line):
                continue
            if not _TODO_MARKER_RE.search(line):
                continue
            if any(s in line for s in _TODO_ALLOWLIST_SUBSTRINGS):
                continue
            # A marker token inside a user-facing message string (a raise/error or
            # a diagnostic kwarg) is guidance text, not a code shortcut.
            if _MESSAGE_STRING_RE.search(line) and "status:" not in line:
                continue
            # A marker whose path FAILS CLOSED (raises within a few lines) is honest,
            # not todo-as-plan poison: it refuses the unsupported input with a precise
            # diagnostic instead of shipping a divergent answer. Exclude it.
            following = "\n".join(lines[i : min(i + 6, len(lines))])
            if re.search(r"\braise\s+\w+", following):
                continue
            # Poison only when the marker sits INSIDE the body of a public shipped
            # def/method (a per-call silent divergence). A module-level marker is an
            # honest scope declaration on the module, not a claim-then-diverge.
            enclosing = _enclosing_public_def(lines, i)
            if enclosing is not None:
                found.append(
                    DiscoveredSite(
                        "C",
                        "todo_as_plan",
                        rel,
                        line.strip(),
                        i + 1,
                        f"todo/hack marker inside shipped public surface {enclosing!r}",
                    )
                )
    return found


# ─────────────────────────────────────────────────────────────────────────────
# Scan D — duplicate authority (two live copies of the same CPython header)
# ─────────────────────────────────────────────────────────────────────────────

# The two authority trees that ship CPython public headers. A substantive header
# present in BOTH is a duplicate authority: the C-API contract now has two homes
# that must be hand-synced, and one of them is always about to drift.
_DUP_AUTHORITY_TREES = (
    "include",  # includes include/ root and include/molt/
    "runtime/molt-cpython-abi/include",
)
_MOLT_INCLUDE_TREE = "include"
_ABI_INCLUDE_TREE = "runtime/molt-cpython-abi/include"

# Only CPython public headers count for the duplicate-authority class. numpy /
# scipy package headers are the ecosystem_baked class (Scan A), not this one.
_CPYTHON_HEADER_NAMES = frozenset(
    {
        "Python.h",
        "abstract.h",
        "object.h",
        "pyerrors.h",
        "pymem.h",
        "structmember.h",
        "frameobject.h",
        "datetime.h",
        "traceback.h",
        "compile.h",
        "pythread.h",
        "ceval.h",
        "import.h",
        "modsupport.h",
    }
)

# A header short enough to be a pure `#include`/guard forwarder is not itself an
# authority copy — it delegates. Substantive copies (real declarations) collide.
_FORWARDER_LINE_CAP = 20


def _molt_side_headers(root: Path) -> dict[str, Path]:
    out: dict[str, Path] = {}
    base = root / _MOLT_INCLUDE_TREE
    if not base.exists():
        return out
    for path in _walk_files(base, frozenset({".h"})):
        if path.suffix != ".h":
            continue
        # Skip the abi tree itself if nested; skip numpy/scipy package dirs.
        posix = path.as_posix()
        if "/numpy/" in posix or "/scipy/" in posix:
            continue
        name = path.name
        if name not in _CPYTHON_HEADER_NAMES:
            continue
        try:
            n = len(path.read_text(encoding="utf-8", errors="replace").splitlines())
        except OSError:
            continue
        if n <= _FORWARDER_LINE_CAP:
            continue  # thin forwarder, not an authority copy
        # Prefer the largest substantive copy per basename on the molt side.
        prev = out.get(name)
        if prev is None or n > len(
            prev.read_text(encoding="utf-8", errors="replace").splitlines()
        ):
            out[name] = path
    return out


def discover_duplicate_authority(root: Path) -> list[DiscoveredSite]:
    abi_base = root / _ABI_INCLUDE_TREE
    if not abi_base.exists():
        return []
    abi_names: dict[str, Path] = {}
    for path in _walk_files(abi_base, frozenset({".h"})):
        if path.suffix != ".h":
            continue
        if path.name in _CPYTHON_HEADER_NAMES:
            n = len(path.read_text(encoding="utf-8", errors="replace").splitlines())
            if n > _FORWARDER_LINE_CAP:
                abi_names.setdefault(path.name, path)

    found: list[DiscoveredSite] = []
    for name, molt_path in _molt_side_headers(root).items():
        twin = abi_names.get(name)
        if twin is None:
            continue
        rel = molt_path.relative_to(root).as_posix()
        text = molt_path.read_text(encoding="utf-8", errors="replace")
        # Anchor on the include guard line (stable, unique per header).
        guard = _first_guard(text)
        found.append(
            DiscoveredSite(
                "D",
                "duplicate_authority",
                rel,
                guard,
                _first_line(text, guard),
                f"{name} also lives at {twin.relative_to(root).as_posix()} — "
                "one C-API authority, two hand-synced copies",
            )
        )
    return found


def _first_guard(text: str) -> str:
    for line in text.splitlines():
        m = re.match(r"^\s*#\s*ifndef\s+([A-Za-z_][A-Za-z0-9_]*)", line)
        if m:
            return line.strip()
    # Fall back to the first non-empty line.
    for line in text.splitlines():
        if line.strip():
            return line.strip()
    return ""


def discover_all(root: Path) -> list[DiscoveredSite]:
    return (
        discover_ecosystem_baked(root)
        + discover_ecosystem_build_crutches(root)
        + discover_ecosystem_reimplementations(root)
        + discover_fail_open_abi(root)
        + discover_fail_open_backend_dispatch(root)
        + discover_todo_as_plan(root)
        + discover_duplicate_authority(root)
    )


# ─────────────────────────────────────────────────────────────────────────────
# Registry + gate
# ─────────────────────────────────────────────────────────────────────────────


@dataclass(frozen=True)
class RegistryRow:
    id: str
    cls: str
    file: str
    anchor: str
    resolution_row: str
    justification: str


@dataclass
class Registry:
    rows: list[RegistryRow]
    baseline: dict[str, int]
    by_file_anchor: dict[tuple[str, str], RegistryRow] = field(default_factory=dict)

    def __post_init__(self) -> None:
        for row in self.rows:
            self.by_file_anchor.setdefault((row.file, row.anchor), row)


@dataclass
class Violation:
    kind: str
    detail: str

    def __str__(self) -> str:
        return f"[{self.kind}] {self.detail}"


def check_research_quarantines(root: Path = REPO_ROOT) -> list[Violation]:
    """Enforce that retained package-semantics clones remain research-only."""

    violations: list[Violation] = []
    for root_rel, package in _QUARANTINED_REFERENCE_ROOTS.items():
        quarantine_root = root / root_rel
        if not quarantine_root.exists():
            continue
        marker = quarantine_root / _RESEARCH_QUARANTINE_MARKER
        if not marker.is_file():
            violations.append(
                Violation(
                    "missing-research-quarantine-marker",
                    f"{root_rel} exists but lacks {_RESEARCH_QUARANTINE_MARKER}; "
                    "retained package-semantics clones must be explicitly marked "
                    "research/reference-only.",
                )
            )
        package_root = quarantine_root / package
        if not package_root.is_dir():
            violations.append(
                Violation(
                    "invalid-research-quarantine-root",
                    f"{root_rel} is registered as a quarantined {package!r} "
                    f"reference but {root_rel}/{package} is missing.",
                )
            )

    for path in _iter_quarantine_text_files(root):
        rel = path.relative_to(root).as_posix()
        if rel in _QUARANTINE_TEXT_ALLOWLIST:
            continue
        if any(
            _is_under_rel(rel, root_rel) for root_rel in _QUARANTINED_REFERENCE_ROOTS
        ):
            continue
        try:
            text = path.read_text(encoding="utf-8", errors="replace")
        except OSError:
            continue
        for match in _QUARANTINED_REFERENCE_USE_RE.finditer(text):
            token = match.group(0)
            if _allowed_quarantined_reference_use(rel, token):
                continue
            violations.append(
                Violation(
                    "research-quarantine-usage",
                    f"{rel} references quarantined package-semantics clone token "
                    f"{token!r}. Use upstream package custody for package "
                    "semantics; only tests/helpers/tinygrad_stdlib_loader.py may "
                    "explicitly load the reference clone for research/regression.",
                )
            )
            break
    return violations


def load_registry(path: Path = REGISTRY_PATH) -> Registry:
    data = tomllib.loads(path.read_text(encoding="utf-8"))
    baseline_tbl = data.get("baseline", {})
    baseline = {cls: int(baseline_tbl.get(cls, 0)) for cls in _VALID_CLASSES}
    rows: list[RegistryRow] = []
    for entry in data.get("site", []):
        rows.append(
            RegistryRow(
                id=entry["id"],
                cls=entry["class"],
                file=entry["file"],
                anchor=entry["anchor"],
                resolution_row=entry.get("resolution_row", "").strip(),
                justification=entry.get("justification", "").strip(),
            )
        )
    return Registry(rows=rows, baseline=baseline)


def check_anchor_present(root: Path, row: RegistryRow) -> Violation | None:
    path = root / row.file
    if not path.exists():
        return Violation(
            "missing-file",
            f"row {row.id!r} names {row.file} which does not exist",
        )
    text = path.read_text(encoding="utf-8", errors="replace")
    if row.anchor not in text:
        return Violation(
            "anchor-drift",
            f"row {row.id!r}: anchor {row.anchor!r} no longer present in "
            f"{row.file}; the site moved or was ripped — reconcile the registry "
            "(and lower the class baseline if it was ripped).",
        )
    return None


def _row_matches_site(registry: Registry, site: DiscoveredSite) -> RegistryRow | None:
    exact = registry.by_file_anchor.get((site.file, site.signature))
    if exact is not None:
        return exact
    # Loose match: a row in the same file whose anchor is a substring of the
    # discovered signature (covers whitespace / trailing-token variance).
    for row in registry.rows:
        if (
            row.file == site.file
            and row.cls == site.cls
            and row.anchor in site.signature
        ):
            return row
    return None


def run_gate(
    root: Path = REPO_ROOT, registry_path: Path | None = None
) -> list[Violation]:
    """Return the list of gate violations (empty == green). ``root`` is the tree
    scanned and the base for anchor resolution; ``registry_path`` defaults to the
    checked-in registry. Both are parameters so the negative-control test can drive
    the gate against a synthetic tree + registry."""

    violations: list[Violation] = []
    registry = load_registry(registry_path or REGISTRY_PATH)

    # --- Registry self-consistency --------------------------------------------
    seen_ids: set[str] = set()
    for row in registry.rows:
        if row.cls not in _VALID_CLASSES:
            violations.append(
                Violation(
                    "bad-class",
                    f"row {row.id!r} has class {row.cls!r}; must be one of "
                    f"{sorted(_VALID_CLASSES)}",
                )
            )
        if row.id in seen_ids:
            violations.append(
                Violation("duplicate-id", f"registry id {row.id!r} is duplicated")
            )
        seen_ids.add(row.id)
        if not row.resolution_row:
            violations.append(
                Violation(
                    "missing-resolution-row",
                    f"row {row.id!r} must name a resolution_row (the rip task "
                    "driving it to zero)",
                )
            )
        if not row.justification:
            violations.append(
                Violation(
                    "missing-justification",
                    f"row {row.id!r} has no justification",
                )
            )

    # --- Anchor integrity: every row's anchor still exists ---------------------
    for row in registry.rows:
        v = check_anchor_present(root, row)
        if v is not None:
            violations.append(v)

    # --- Drift: every discovered poison site must be registered ----------------
    discovered = discover_all(root)
    for site in discovered:
        if _row_matches_site(registry, site) is None:
            violations.append(
                Violation(
                    "unregistered-poison-site",
                    f"[{site.cls}] {site.file}:{site.line} introduces poison site "
                    f"{site.signature!r} ({site.detail}) with no registry row. Add a "
                    "row to tools/fail_closed_registry.toml classifying it, or rip "
                    "the site. New poison MUST NOT land unregistered.",
                )
            )

    # --- Research quarantine: retained package clones must stay isolated -------
    violations.extend(check_research_quarantines(root))

    # --- Ratchet: per-class registered count is monotonically non-increasing ---
    live_counts: dict[str, int] = {cls: 0 for cls in _VALID_CLASSES}
    for row in registry.rows:
        if row.cls in live_counts:
            live_counts[row.cls] += 1
    for cls in sorted(_VALID_CLASSES):
        if live_counts[cls] > registry.baseline[cls]:
            violations.append(
                Violation(
                    "ratchet-regression",
                    f"{live_counts[cls]} {cls!r} rows exceed baseline "
                    f"{registry.baseline[cls]}. This poison class must only "
                    "shrink. Rip a site (and lower the baseline) rather than "
                    "registering new poison.",
                )
            )

    return violations


def main(argv: list[str] | None = None) -> int:
    argv = sys.argv[1:] if argv is None else argv
    if "--discover" in argv:
        # Debug aid: print the raw discovered set (not a gate pass/fail).
        for site in discover_all(REPO_ROOT):
            print(f"[{site.scan}/{site.cls}] {site.file}:{site.line}\t{site.signature}")
        return 0

    violations = run_gate(REPO_ROOT)
    if violations:
        print("fail-closed gate: FAIL", file=sys.stderr)
        for v in violations:
            print(f"  {v}", file=sys.stderr)
        print(
            f"\n{len(violations)} violation(s). tools/fail_closed_registry.toml is "
            "the authority; register or rip the site there.",
            file=sys.stderr,
        )
        return 1

    registry = load_registry(REGISTRY_PATH)
    counts = {cls: 0 for cls in _VALID_CLASSES}
    for row in registry.rows:
        counts[row.cls] += 1
    summary = ", ".join(
        f"{cls}={counts[cls]}<={registry.baseline[cls]}"
        for cls in sorted(_VALID_CLASSES)
    )
    print(f"fail-closed gate: OK ({len(registry.rows)} registered sites; {summary})")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
