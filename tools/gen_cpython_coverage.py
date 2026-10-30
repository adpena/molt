#!/usr/bin/env python3
from __future__ import annotations

import re
import tomllib
from collections import Counter
from pathlib import Path

import sys

_REPO_ROOT = Path(__file__).resolve().parents[1]
if str(_REPO_ROOT) not in sys.path:
    sys.path.insert(0, str(_REPO_ROOT))

from tools.import_file import bind_repository_imports  # noqa: E402

bind_repository_imports(__file__)

from tools.command_execution import CommandExecutor  # noqa: E402
from tools.generator_io import generator_main  # noqa: E402
from molt.source_admission import repository_source_paths  # noqa: E402
from tools.scan_records import ScanRecords, decoded_text, scan_result  # noqa: E402

_COMMANDS = CommandExecutor.for_file(__file__)

ROOT = Path(__file__).resolve().parents[1]
CONFIG = ROOT / "config" / "cpython_coverage.toml"
STABLE_ABI = ROOT / "config" / "cpython_stable_abi_3_12.toml"
DOC = (
    ROOT
    / "docs"
    / "spec"
    / "areas"
    / "compat"
    / "surfaces"
    / "cpython_abi_coverage.generated.md"
)
AUDIT = (
    ROOT
    / "docs"
    / "spec"
    / "areas"
    / "compat"
    / "surfaces"
    / "cpython_version_assumptions.generated.md"
)
SOURCE_ROOT = ROOT / "runtime" / "molt-cpython-abi"
AUDIT_SUFFIXES = frozenset({".py", ".rs", ".c", ".h", ".toml"})

# This is a lexical inventory, independent of supported target-version admission.
# Include older/future minors; reject embedded decimal and malformed hex words.
VERSION_PATTERN = re.compile(
    r"(?<![0-9.])3\.[0-9]+"
    r"|\b(?:[A-Za-z_]\w*\.)?version_info\b|\bPY_VERSION_HEX\b"
    r"|\b0[xX]03[0-9A-Fa-f]{6}\b"
)
DECL_PATTERN = re.compile(
    r"#\[unsafe\(no_mangle\)\]\s*"
    r"(?:pub\s+)?(?:(?:unsafe)\s+)?(?:extern\s+\"C\"\s+)?"
    r"(fn|static(?:\s+mut)?)\s+([A-Za-z_][A-Za-z0-9_]*)",
    re.MULTILINE,
)


def _load_config() -> dict[str, object]:
    return tomllib.loads(CONFIG.read_text(encoding="utf-8"))


def _audit_source_roots(config: dict[str, object]) -> tuple[Path, ...]:
    audit = config.get("version_assumption_audit")
    if not isinstance(audit, dict):
        raise ValueError("missing [version_assumption_audit] configuration")
    source_roots = audit.get("source_roots")
    if not isinstance(source_roots, list) or not source_roots:
        raise ValueError(
            "version_assumption_audit.source_roots must be a non-empty list"
        )

    repo_root = ROOT.resolve()
    roots: list[Path] = []
    seen: set[Path] = set()
    for value in source_roots:
        if not isinstance(value, str) or not value:
            raise ValueError(
                "version_assumption_audit.source_roots entries must be strings"
            )
        relative = Path(value)
        if relative.is_absolute() or ".." in relative.parts:
            raise ValueError(f"audit source root must be repo-relative: {value!r}")
        root = (ROOT / relative).resolve()
        if root != repo_root and repo_root not in root.parents:
            raise ValueError(
                f"audit source root resolves outside the repository: {value!r}"
            )
        if root in seen:
            raise ValueError(f"duplicate audit source root: {value!r}")
        overlap = next(
            (
                existing
                for existing in roots
                if root in existing.parents or existing in root.parents
            ),
            None,
        )
        if overlap is not None:
            raise ValueError(
                "overlapping audit source roots would scan files more than once: "
                f"{overlap.relative_to(repo_root)} and {value!r}"
            )
        if not root.is_dir():
            raise ValueError(f"audit source root does not exist: {value!r}")
        seen.add(root)
        roots.append(root)
    return tuple(roots)


def _stable_abi_symbols() -> set[str]:
    data = tomllib.loads(STABLE_ABI.read_text(encoding="utf-8"))
    return {str(name) for kind in ("function", "data") for name in data.get(kind, {})}


def _stability(symbol: str, stable_abi: set[str] | None = None) -> str:
    if symbol.startswith("_Py"):
        return "private"
    if symbol.startswith("PyUnstable_"):
        return "unstable"
    if symbol.startswith("molt_"):
        return "molt-private"
    if symbol in (_stable_abi_symbols() if stable_abi is None else stable_abi):
        return "stable-abi"
    return "public-version-specific"


# ABI surfaces that are not exported symbols (struct layouts, flag values and
# module slots), each with its stability class.
STRUCTURAL_SURFACES = (
    ("PyObject.layout", "public-version-specific"),
    ("PyTypeObject.layout", "public-version-specific"),
    ("Py_TPFLAGS", "stable-values-version-specific-layout"),
    ("PyModuleDef_Slot", "stable-abi"),
    ("object-header.immortal-refcount", "private-representation"),
)


def _export_stabilities() -> dict[str, str]:
    """Each exported C symbol mapped to its stability class."""
    stable_abi = _stable_abi_symbols()
    symbols: set[str] = set()
    for path in sorted((SOURCE_ROOT / "src").rglob("*.rs")):
        text = path.read_text(encoding="utf-8")
        symbols.update(match.group(2) for match in DECL_PATTERN.finditer(text))
    exports_file = SOURCE_ROOT / "shims" / "pyarg_variadic.exports"
    for raw in exports_file.read_text(encoding="utf-8").splitlines():
        symbol = raw.strip()
        if symbol and not symbol.startswith("#"):
            symbols.add(symbol)
    return {symbol: _stability(symbol, stable_abi) for symbol in sorted(symbols)}


def _render_doc(config: dict[str, object]) -> str:
    counts = Counter(_export_stabilities().values())
    counts.update(stability for _surface, stability in STRUCTURAL_SURFACES)
    lines = [
        "# CPython ABI Coverage (generated)",
        "",
        "Generated by `tools/gen_cpython_coverage.py` from "
        "`config/cpython_coverage.toml`; do not edit by hand.",
        "",
        "## Verified boundary",
        "",
    ]
    for row in config.get("verified", []):
        lines.append(
            f"- CPython {row['cpython']} on {row['platform']}: {row['evidence']}"
        )
    lines.extend(["", "## Unverified candidates", ""])
    for row in config.get("candidate", []):
        lines.append(
            f"- CPython {row['cpython']} on {row['platform']} ({row['status']}): "
            f"{row['evidence']}"
        )
    lines.extend(["", "## Counts", "", f"- Total surfaces: {counts.total()}"])
    for key, value in sorted(counts.items()):
        lines.append(f"- `{key}`: {value}")
    lines.extend(["", "## Known divergences in later CPython versions", ""])
    for row in config.get("divergence", []):
        lines.append(
            f"- `{row['surface']}` (CPython {row['introduced']}): {row['summary']} "
            f"Source: <{row['source']}>"
        )
    lines.extend(
        [
            "",
            "## Contract",
            "",
            "The counts cover every exported C symbol discovered from `#[unsafe(no_mangle)]` declarations and the variadic shim export list, plus the structural layout, flag, and module-slot surfaces. Public symbols are conservatively classified as version-specific unless a stronger stable-ABI contract is explicitly represented; `_Py*` is private and `PyUnstable_*` is unstable.",
            "",
            "The divergence list records 3.13/3.14 surfaces for future work; it does not claim support for those versions.",
        ]
    )
    return "\n".join(lines) + "\n"


def _read_source_git(cwd: Path, arguments: list[str]) -> bytes:
    return _COMMANDS.run(
        ["git", *arguments], cwd=cwd, capture_output=True, timeout=30, check=True
    ).stdout


def _audit_sources(bases: tuple[Path, ...]) -> list[Path]:
    """Git-admitted sources under each of ``bases``, in configuration order.

    Git owns the ignore rules, so build output inside a source root (a Cargo
    target, a trybuild tree) never enters the projection and every host
    generates the same rows.
    """
    admitted = [
        path
        for path in repository_source_paths(
            ROOT, run_git=_read_source_git, include_untracked=True
        )
        if path.suffix in AUDIT_SUFFIXES
    ]
    # Each path's ancestors are built once. Path.is_relative_to rebuilds them
    # for every (base, path) pair; set membership keeps its equality rule.
    ancestors = {path: {path, *path.parents} for path in admitted}
    return [
        path
        for base in bases
        for path in sorted(path for path in admitted if base in ancestors[path])
    ]


def _audit_row(text: str, configured: bool) -> list[object] | None:
    """One audited file's lexical row: a pure function of its text."""
    matches = VERSION_PATTERN.finditer(text)
    first_match = next(matches, None)
    if first_match is None:
        return None
    match_count = 1 + sum(1 for _match in matches)
    routed = "TargetPythonVersion" in text or "target_python" in text or configured
    operational = (
        "uv run --python" in text
        or "PYTHON_VERSION" in text
        or "sys.version_info" in text
    )
    classification = (
        "authority-routed"
        if routed
        else ("operational-tooling" if operational else "scattered-assumption")
    )
    return [match_count, classification, first_match.group(0)]


def _audit(config: dict[str, object], records: ScanRecords | None = None) -> str:
    rows: list[tuple[str, int, str, str]] = []
    for path in _audit_sources(_audit_source_roots(config)):
        data = path.read_bytes()
        configured = path == CONFIG
        row = scan_result(
            records,
            f"audit-row:{configured}",
            data,
            lambda: _audit_row(
                decoded_text(data, encoding="utf-8", errors="replace"), configured
            ),
            encode=lambda value: value,
            decode=lambda value: value,
        )
        if row is None:
            continue
        match_count, classification, first = row
        rows.append(
            (
                path.relative_to(ROOT).as_posix(),
                int(match_count),
                str(classification),
                str(first),
            )
        )
    totals = Counter(row[2] for row in rows)
    lines = [
        "# CPython Version-Assumption Audit (generated)",
        "",
        "Generated by `tools/gen_cpython_coverage.py`; file and match counts are raw lexical hits.",
        "",
        "The inventory includes older and future Python 3 minor spellings, full eight-digit [`0x03......` words](https://github.com/python/cpython/blob/v3.15.0/Include/patchlevel.h#L29-L42), `version_info` attribute/import spellings, and `PY_VERSION_HEX`. Unrelated decimal numbers, hex words and identifiers can match. File-level classifications use text markers only; they do not prove authority routing or semantic conformance. Supported targets remain declared by `src/molt/target_python_policy.py`.",
        "",
        "## Counts",
        "",
    ]
    for key, value in sorted(totals.items()):
        lines.append(f"- `{key}` files: {value}")
    lines.extend(
        [
            "",
            "## Files",
            "",
            "| File | Matches | Classification | First match |",
            "|---|---:|---|---|",
        ]
    )
    lines.extend(
        f"| `{path}` | {count} | {kind} | `{first}` |"
        for path, count, kind, first in rows
    )
    return "\n".join(lines) + "\n"


def _outputs(
    config: dict[str, object] | None = None, records: ScanRecords | None = None
) -> dict[Path, str]:
    resolved_config = _load_config() if config is None else config
    return {
        DOC: _render_doc(resolved_config),
        AUDIT: _audit(resolved_config, records),
    }


def generated_outputs(*, scan_records: ScanRecords | None = None) -> dict[Path, str]:
    """Each output path mapped to its exact generated text.

    ``tools/generators.py`` passes its scan records; per-file audit rows replay.
    """
    return dict(_outputs(records=scan_records))


def main(argv: list[str] | None = None) -> int:
    return generator_main(generated_outputs, argv, description=__doc__)


if __name__ == "__main__":
    raise SystemExit(main())
