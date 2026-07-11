#!/usr/bin/env python3
"""Encoding-safety gate — a deterministic guard against the Windows cp1252 bug class.

THE BUG CLASS THIS PREVENTS
---------------------------
On Windows the default text codec is cp1252 (``locale.getpreferredencoding()``),
NOT UTF-8. Any text I/O that relies on that default silently does the wrong thing
the moment a non-cp1252 character appears — and, worse, *aborts* the process with
``UnicodeEncodeError``/``UnicodeDecodeError``. A real incident: a ~20-minute
witness build succeeded, then a tool ``print()``-relayed captured subprocess
stdout that contained an em-dash (decoded to U+FFFD); the default codec crashed
and threw the whole build away. (M43 — the recurring cp1252 encoding bug class.)

The fix is boring and universal: **always name the encoding**. This gate makes
"forgot the encoding" a machine-checked, fail-closed event instead of a landmine.

WHAT IT FLAGS (AST-based, not regex, for correctness)
-----------------------------------------------------
Three encoding-unsafe patterns whose default is the platform codec:

  * ``open(...)`` / ``io.open(...)`` opened in TEXT mode (no ``'b'`` in the mode)
    WITHOUT an ``encoding=`` keyword — rule ``open-no-encoding``.
  * ``<x>.read_text(...)`` / ``<x>.write_text(...)`` (``pathlib.Path``) WITHOUT an
    ``encoding=`` keyword — rules ``read_text-no-encoding`` / ``write_text-no-encoding``.
  * ``subprocess.run/Popen/check_output/check_call(..., text=True)`` (or
    ``universal_newlines=True``) WITHOUT an ``encoding=`` keyword — rule
    ``subprocess-text-no-encoding``. In text mode subprocess decodes child bytes
    with the platform codec; a child that emits UTF-8 (or anything non-cp1252)
    then raises on read — exactly the witness-build failure.

Conservative by construction (avoids false positives): a call that forwards
``**kwargs`` may carry ``encoding`` we can't see, so it is NOT flagged; a
``text=<variable>`` (non-literal) is NOT flagged; an explicit ``encoding=None``
IS flagged (it does not actually pin an encoding — it re-selects the default).

SCOPE
-----
First-party, highest-traffic Python that relays process/file text:
``tools/**/*.py`` and ``src/molt/**/*.py``. (Vendored/generated/``target`` trees
are excluded.) This is the surface where a relayed cp1252 crash poisons a build.

RATCHET
-------
Same monotonic-non-increasing shape as ``tools/dead_code_allow_ratchet.py``: the
committed baseline count may only DECREASE (burn down to zero — the goal), never
rise. The baseline is fingerprinted by ``file::rule`` (line-number independent)
so an unrelated edit that shifts lines does not churn it, and so a fixed
violation cannot be silently replaced by a brand-new one at net-zero count.

Usage:
    python tools/encoding_gate.py            # or --check: fail if NEW violations
    python tools/encoding_gate.py --list     # print every current violation
    python tools/encoding_gate.py --update   # rebaseline (review the drop/rise)

Exit: 0 = at-or-below baseline; 2 = a NEW violation (fix it, or --update with
justification); 3 = usage/IO error.
"""

from __future__ import annotations

import argparse
import ast
import json
from pathlib import Path
import sys

# Dogfood the runtime backstop: this gate prints subprocess-style diagnostics and
# runs inside ci_gate (high traffic), so it must not itself trip the very cp1252
# crash it guards against. force_utf8_stdio() is idempotent and never raises.
try:  # importable whether launched as a script (tools/ on path) or as tools.X
    from _io_utf8 import force_utf8_stdio
except ModuleNotFoundError:  # pragma: no cover
    from tools._io_utf8 import force_utf8_stdio
force_utf8_stdio()

ROOT = Path(__file__).resolve().parents[1]
BASELINE = Path(__file__).resolve().parent / "encoding_gate_baseline.json"

# Roots scanned for first-party Python sources (relative to repo ROOT).
_SCAN_ROOTS = ("tools", "src/molt")
# Directory names that are never first-party source (vendored/build/generated).
_SKIP_DIR_PARTS = frozenset(
    {".git", "target", "__pycache__", ".venv", "node_modules", "vendor", ".mypy_cache"}
)

_SUBPROCESS_TEXT_FUNCS = frozenset({"run", "Popen", "check_output", "check_call"})


class Violation:
    """One encoding-unsafe call site."""

    __slots__ = ("relpath", "line", "rule")

    def __init__(self, relpath: str, line: int, rule: str) -> None:
        self.relpath = relpath
        self.line = line
        self.rule = rule

    @property
    def fingerprint(self) -> str:
        """Line-number-independent identity: same site survives unrelated edits."""
        return f"{self.relpath}::{self.rule}"

    def location(self) -> str:
        return f"{self.relpath}:{self.line}"


def _kwarg(call: ast.Call, name: str) -> ast.keyword | None:
    for kw in call.keywords:
        if kw.arg == name:
            return kw
    return None


def _has_double_star(call: ast.Call) -> bool:
    """True if the call forwards ``**kwargs`` (encoding may be hidden inside)."""
    return any(kw.arg is None for kw in call.keywords)


def _encoding_is_pinned(call: ast.Call) -> bool:
    """True if the call explicitly names a real (non-None) encoding."""
    kw = _kwarg(call, "encoding")
    if kw is None:
        return False
    # ``encoding=None`` does NOT pin an encoding — it re-selects the platform
    # default, which is the very bug this gate exists to stop.
    return not (isinstance(kw.value, ast.Constant) and kw.value.value is None)


def _mode_string(call: ast.Call) -> str | None:
    """Return the literal mode string for an ``open`` call, or None if absent/non-literal.

    ``mode`` is the 2nd positional arg or the ``mode=`` keyword.
    """
    mode_node: ast.expr | None = None
    if len(call.args) >= 2:
        mode_node = call.args[1]
    else:
        kw = _kwarg(call, "mode")
        if kw is not None:
            mode_node = kw.value
    if mode_node is None:
        return ""  # mode omitted -> defaults to text mode "r"
    if isinstance(mode_node, ast.Constant) and isinstance(mode_node.value, str):
        return mode_node.value
    return None  # non-literal mode -> cannot prove text mode; caller stays silent


def _is_open_call(call: ast.Call) -> bool:
    """``open(...)`` (builtin) or ``io.open(...)``."""
    func = call.func
    if isinstance(func, ast.Name):
        return func.id == "open"
    if isinstance(func, ast.Attribute) and func.attr == "open":
        return isinstance(func.value, ast.Name) and func.value.id == "io"
    return False


def _bool_true(node: ast.expr | None) -> bool:
    return isinstance(node, ast.Constant) and node.value is True


def _called_name(func: ast.expr) -> str | None:
    """The simple callable name for a ``foo(...)`` or ``x.foo(...)`` call."""
    if isinstance(func, ast.Name):
        return func.id
    if isinstance(func, ast.Attribute):
        return func.attr
    return None


def _check_call(call: ast.Call, relpath: str, out: list[Violation]) -> None:
    func = call.func

    # Rule 1: open(...) / io.open(...) in text mode without encoding=.
    if _is_open_call(call):
        mode = _mode_string(call)
        if mode is not None and "b" not in mode:  # text mode (proven or default)
            if not _has_double_star(call) and not _encoding_is_pinned(call):
                out.append(Violation(relpath, call.lineno, "open-no-encoding"))
        return

    name = _called_name(func)
    if name is None:
        return

    # Rule 2: Path.read_text / Path.write_text without encoding=. These are always
    # bound methods, so require the attribute form (a bare read_text() is not a
    # thing) to avoid false positives on unrelated same-named functions.
    if name in ("read_text", "write_text") and isinstance(func, ast.Attribute):
        if not _has_double_star(call) and not _encoding_is_pinned(call):
            out.append(Violation(relpath, call.lineno, f"{name}-no-encoding"))
        return

    # Rule 3: subprocess.<run|Popen|check_output|check_call>(..., text=True) or
    # (..., universal_newlines=True) without encoding=. Accept both `subprocess.run`
    # and a bare `run` (`from subprocess import run` is idiomatic). Gate on the
    # text-mode kwarg being a literal True -- a non-literal is not provably text
    # mode, and `text=True` is subprocess-specific enough to bound false positives.
    if name in _SUBPROCESS_TEXT_FUNCS:
        text_kw = _kwarg(call, "text")
        univ_kw = _kwarg(call, "universal_newlines")
        text_mode = _bool_true(text_kw.value if text_kw else None) or _bool_true(
            univ_kw.value if univ_kw else None
        )
        if text_mode and not _has_double_star(call) and not _encoding_is_pinned(call):
            out.append(Violation(relpath, call.lineno, "subprocess-text-no-encoding"))
        return


def scan_source(source: str, relpath: str) -> list[Violation]:
    """All violations in a single source string. Pure — the unit the teeth test drives."""
    try:
        tree = ast.parse(source, filename=relpath)
    except SyntaxError:
        return []  # not valid Python for this interpreter; skip, don't crash the gate
    out: list[Violation] = []
    for node in ast.walk(tree):
        if isinstance(node, ast.Call):
            _check_call(node, relpath, out)
    return out


def _scan_file(path: Path, relpath: str) -> list[Violation]:
    try:
        source = path.read_text(encoding="utf-8", errors="replace")
    except OSError:
        return []
    return scan_source(source, relpath)


def _iter_python_files() -> list[tuple[Path, str]]:
    files: list[tuple[Path, str]] = []
    for root_name in _SCAN_ROOTS:
        root = ROOT / root_name
        if not root.is_dir():
            continue
        for py in root.rglob("*.py"):
            if _SKIP_DIR_PARTS & set(py.parts):
                continue
            files.append((py, py.relative_to(ROOT).as_posix()))
    return files


def scan() -> list[Violation]:
    """All current violations across the scanned tree, sorted for stable output."""
    out: list[Violation] = []
    for path, relpath in _iter_python_files():
        out.extend(_scan_file(path, relpath))
    out.sort(key=lambda v: (v.relpath, v.line, v.rule))
    return out


def _counts(violations: list[Violation]) -> dict[str, int]:
    """Per-site (``file::rule``) violation counts — line-number independent.

    Keying by ``file::rule`` (not by line) means an unrelated edit that shifts
    lines does not churn the baseline, while a per-key COUNT (not a mere set)
    still catches an ADDITIONAL unsafe call inside a file that already trips that
    rule — a hole a fingerprint set would miss.
    """
    counts: dict[str, int] = {}
    for v in violations:
        counts[v.fingerprint] = counts.get(v.fingerprint, 0) + 1
    return counts


def _baseline_payload(violations: list[Violation]) -> dict[str, object]:
    counts = _counts(violations)
    return {
        "total": sum(counts.values()),
        "note": (
            "encoding-unsafe call sites keyed file::rule; each count is monotonic "
            "non-increasing (burn down to zero, never up). See tools/encoding_gate.py."
        ),
        "by_site": dict(sorted(counts.items())),
    }


def regressions(counts: dict[str, int], base_by_site: dict[str, int]) -> list[str]:
    """Sorted file::rule keys whose count rose above baseline.

    A brand-new ``file::rule`` has baseline 0, so any occurrence trips it; an
    existing key trips on +1. This is strictly stronger than a total-count
    ratchet — it cannot be fooled by fixing one site and adding another.
    """
    return sorted(fp for fp, c in counts.items() if c > base_by_site.get(fp, 0))


def _load_baseline() -> dict[str, object] | None:
    if not BASELINE.is_file():
        return None
    try:
        return json.loads(BASELINE.read_text(encoding="utf-8"))
    except (OSError, ValueError):
        return None


def _write_baseline(violations: list[Violation]) -> None:
    BASELINE.write_text(
        json.dumps(_baseline_payload(violations), indent=2) + "\n",
        encoding="utf-8",
    )


def _cmd_list(violations: list[Violation]) -> int:
    if not violations:
        print("encoding_gate: no violations -- clean tree.")
        return 0
    for v in violations:
        print(f"{v.location()}: {v.rule}")
    print(f"\nencoding_gate: {len(violations)} violation(s).")
    return 0


def _cmd_update(violations: list[Violation]) -> int:
    _write_baseline(violations)
    print(f"encoding_gate: baseline updated to total={len(violations)} site(s)")
    return 0


def _cmd_check(violations: list[Violation]) -> int:
    base = _load_baseline()
    if base is None:
        print(
            "encoding_gate: no baseline -- run --update once to establish it.",
            file=sys.stderr,
        )
        return 3

    base_by_site: dict[str, int] = {
        str(k): int(v) for k, v in dict(base.get("by_site", {})).items()
    }
    base_total = int(base.get("total", sum(base_by_site.values())))
    counts = _counts(violations)
    total = sum(counts.values())
    by_fp: dict[str, Violation] = {v.fingerprint: v for v in violations}
    risen = regressions(counts, base_by_site)

    if risen:
        print(
            f"encoding_gate: FAIL -- {len(risen)} encoding-unsafe call site(s) rose "
            f"above baseline:",
            file=sys.stderr,
        )
        for fp in risen:
            was = base_by_site.get(fp, 0)
            now = counts[fp]
            delta = "NEW" if was == 0 else f"{was}->{now}"
            print(
                f"    {by_fp[fp].location()}: {by_fp[fp].rule}  ({delta})",
                file=sys.stderr,
            )
        print(
            '\nName the encoding explicitly (encoding="utf-8") on the flagged call, '
            "or open the file in binary mode (text=... -> add encoding= for "
            "subprocess). If the finding is genuinely a false positive, rebaseline "
            "with `python tools/encoding_gate.py --update` and justify it in the "
            "commit. Goal: burn the baseline DOWN to zero, never up.",
            file=sys.stderr,
        )
        return 2

    if total < base_total:
        print(
            f"encoding_gate: PASS -- {total} <= baseline {base_total} "
            f"(down {base_total - total} -- nice; run --update to lock it in)."
        )
        return 0
    print(f"encoding_gate: PASS -- {total} <= baseline {base_total}.")
    return 0


def main(argv: list[str] | None = None) -> int:
    ap = argparse.ArgumentParser(description=__doc__)
    group = ap.add_mutually_exclusive_group()
    group.add_argument(
        "--check",
        action="store_true",
        help="fail if any NEW violation appears vs the baseline (default action)",
    )
    group.add_argument(
        "--list", action="store_true", help="print every current violation"
    )
    group.add_argument(
        "--update", action="store_true", help="rebaseline to the current violations"
    )
    args = ap.parse_args(argv)

    violations = scan()

    if args.list:
        return _cmd_list(violations)
    if args.update:
        return _cmd_update(violations)
    return _cmd_check(violations)


if __name__ == "__main__":
    raise SystemExit(main())
