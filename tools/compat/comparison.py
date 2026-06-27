"""The single CPython-parity comparison law (doc 66 Phase 0 / FACT 2).

Before this module existed there were TWO comparison laws in the tree:
  * tests/molt_diff.py inlined `_canonicalize_stdout` / `_stderr_matches`
    (byte-exact stdout + exception-signature stderr + exact exit code), and
  * tools/parity_gate.py re-implemented its own 3-tier STRICT/RELAXED/EXCLUDED
    comparison (`compare` / `_normalize_relaxed`).

doc 66 §1.1 names that dual truth as the exact "two parity runners disagree"
anti-pattern the project forbids. This module is the structural fix: it is the
ONE place the comparison law lives, imported by both molt_diff.py (verbatim
behaviour — the byte-for-byte regression in tests/test_compat_comparison.py
proves the law moved without changing) and parity_gate.py (whose 3-tier logic is
now a *mode* of this one law). It is also consumed by the multi-backend oracle
(`--target` in molt_diff.py) for the cross-backend divergence sub-oracle, where
two molt backends are compared against each other under the very same rule that
compares one backend against CPython — so a backend fork is judged identically
to a CPython mismatch.

The law has three orthogonal axes, mirroring CPython's observable contract:
  1. stdout  — `canonicalize_stdout` + `stdout_matches`
  2. stderr  — `extract_exception_signature` + `stderr_matches`
  3. exit    — `compare_exit`

and a `ComparisonMode` that selects the stdout strictness:
  * EXACT        — byte-identical stdout (molt_diff default / parity STRICT)
  * PYPERFORMANCE— numeric tokens collapsed (declared pyperformance class)
  * RELAXED      — memory addresses / refcounts normalized (parity RELAXED tier)

No molt-specific or backend-specific branch may live here: the law is the same
for every backend. That is the enforced meaning of "semantic authority is
shared" at the harness layer.
"""

from __future__ import annotations

import re
from dataclasses import dataclass
from enum import Enum

__all__ = [
    "ComparisonMode",
    "ExitLaw",
    "Outputs",
    "Verdict",
    "canonicalize_stdout",
    "normalize_relaxed",
    "extract_exception_signature",
    "stderr_matches",
    "stdout_matches",
    "compare_exit",
    "compare_outputs",
    "outputs_equivalent",
]


class ComparisonMode(str, Enum):
    """How strict the stdout comparison is.

    EXACT is the molt_diff default and the parity STRICT tier; RELAXED is the
    parity RELAXED tier (address/refcount normalization); PYPERFORMANCE collapses
    numeric tokens for the declared pyperformance program class.
    """

    EXACT = "exact"
    RELAXED = "relaxed"
    PYPERFORMANCE = "pyperformance"


class ExitLaw(str, Enum):
    """How exit codes are compared.

    EXACT requires the exact same code (molt_diff's `cp_ret == molt_ret`).
    COMPATIBLE only requires the success/failure partition to agree
    (CPython 0 <=> molt 0; CPython nonzero <=> molt nonzero) — doc 66 §4 codifies
    this as the cross-engine exit-code-compatibility rule that parity_gate.py's
    Oracle-1 documented informally.
    """

    EXACT = "exact"
    COMPATIBLE = "compatible"


# ---------------------------------------------------------------------------
# stdout canonicalization
# ---------------------------------------------------------------------------

# Moved verbatim from tests/molt_diff.py (`_STDOUT_NUMERIC_TOKEN_RE`,
# `_STDOUT_SPACING_RE`, `_canonicalize_stdout`) so the pyperformance class
# behaves byte-identically after the extraction.
_STDOUT_NUMERIC_TOKEN_RE = re.compile(
    r"(?<![A-Za-z0-9_])[-+]?(?:\d+(?:\.\d+)?|\.\d+)(?:[eE][-+]?\d+)?(?![A-Za-z0-9_])"
)
_STDOUT_SPACING_RE = re.compile(r"\s+")

# Moved verbatim from tools/parity_gate.py (`_ADDR_RE`, `_REFCOUNT_RE`,
# `_OBJ_ADDR_RE`, `_normalize_relaxed`) so the RELAXED tier behaves identically
# after being folded in as a mode here.
_ADDR_RE = re.compile(r"\b0x[0-9a-fA-F]+\b")
_REFCOUNT_RE = re.compile(r"\brefcount\s*=\s*\d+", re.IGNORECASE)
_OBJ_ADDR_RE = re.compile(r"(at|id=)\s*0x[0-9a-fA-F]+")


def normalize_relaxed(text: str) -> str:
    """Normalize output for the RELAXED tier (parity_gate Tier 2).

    Strips memory addresses and refcount values that legitimately differ between
    engines/runs but are not semantic content.
    """
    text = _ADDR_RE.sub("0xADDR", text)
    text = _OBJ_ADDR_RE.sub(r"\g<1> 0xADDR", text)
    text = _REFCOUNT_RE.sub("refcount=<N>", text)
    return text


def canonicalize_stdout(
    text: str, mode: str | ComparisonMode = ComparisonMode.EXACT
) -> str:
    """Canonicalize stdout per the comparison mode.

    `mode` accepts the historical per-test `# MOLT_META: stdout=` string values
    ("exact"/""/"pyperformance") as well as a ComparisonMode, so callers that
    forward the raw metadata token need no translation.
    """
    normalized = _mode_token(mode)
    if normalized in {"", "exact"}:
        return text
    if normalized == "pyperformance":
        lines: list[str] = []
        for raw_line in text.splitlines():
            line = raw_line.strip()
            if not line:
                continue
            line = _STDOUT_NUMERIC_TOKEN_RE.sub("<num>", line)
            line = _STDOUT_SPACING_RE.sub(" ", line)
            lines.append(line)
        return "\n".join(lines)
    if normalized == "relaxed":
        return normalize_relaxed(text)
    return text


def stdout_matches(
    cpython_stdout: str,
    molt_stdout: str,
    mode: str | ComparisonMode = ComparisonMode.EXACT,
) -> bool:
    """True iff the two stdout strings match under `mode`."""
    return canonicalize_stdout(cpython_stdout, mode) == canonicalize_stdout(
        molt_stdout, mode
    )


# ---------------------------------------------------------------------------
# stderr / exception-signature law
# ---------------------------------------------------------------------------

# Moved verbatim from tests/molt_diff.py (`_EXCEPTION_SIGNATURE_RE`,
# `_extract_exception_signature`, `_stderr_matches`). Frame/path formatting may
# differ across engines (especially wasm), but exception type+message must be
# exact — the comment at the original site is preserved below.
_EXCEPTION_SIGNATURE_RE = re.compile(
    r"^(?P<etype>[A-Za-z_][A-Za-z0-9_.]*)(?:: (?P<message>.*))?$"
)


def extract_exception_signature(stderr: str) -> tuple[str, str] | None:
    """Return (exception_type, message) from the last signature-shaped line."""
    lines = [line.strip() for line in stderr.splitlines() if line.strip()]
    for line in reversed(lines):
        match = _EXCEPTION_SIGNATURE_RE.match(line)
        if match is None:
            continue
        etype = match.group("etype")
        message = match.group("message") or ""
        return etype, message
    return None


def stderr_matches(cpython_stderr: str, molt_stderr: str, mode: str) -> bool:
    """True iff stderr matches under `mode` (the per-test `stderr=` token).

    Modes: "" / "ignore" -> always True; "match" / "exact" -> byte-exact;
    "traceback" / "exception" / "exception_signature" -> compare exception
    type+message only (frame/path formatting may differ across engines).
    """
    normalized = mode.strip().lower()
    if normalized in {"", "ignore"}:
        return True
    if normalized in {"match", "exact"}:
        return cpython_stderr == molt_stderr
    if normalized in {"traceback", "exception", "exception_signature"}:
        cpython_sig = extract_exception_signature(cpython_stderr)
        molt_sig = extract_exception_signature(molt_stderr)
        if cpython_sig is None or molt_sig is None:
            return cpython_stderr == molt_stderr
        # Frame/path formatting may differ across engines (especially wasm),
        # but exception type/message must remain exact.
        return cpython_sig == molt_sig
    return cpython_stderr == molt_stderr


# ---------------------------------------------------------------------------
# exit-code law
# ---------------------------------------------------------------------------


def compare_exit(
    cpython_rc: int,
    molt_rc: int,
    law: str | ExitLaw = ExitLaw.EXACT,
) -> bool:
    """Compare exit codes under `law`.

    EXACT: identical codes (molt_diff semantics). COMPATIBLE: only the
    success/failure partition must agree (doc 66 §4 cross-engine rule).
    """
    normalized = law.value if isinstance(law, ExitLaw) else str(law).strip().lower()
    if normalized == ExitLaw.COMPATIBLE.value:
        return (cpython_rc == 0) == (molt_rc == 0)
    return cpython_rc == molt_rc


# ---------------------------------------------------------------------------
# Composite verdict
# ---------------------------------------------------------------------------


@dataclass(frozen=True)
class Outputs:
    """One engine's observable result for a program: (stdout, stderr, rc).

    `stdout` is None when the engine never produced output (e.g. a build
    failure on the molt side); the comparison treats None as a hard mismatch
    unless the caller resolves the build-failure case first (as diff_test does).
    """

    stdout: str | None
    stderr: str
    returncode: int


@dataclass(frozen=True)
class Verdict:
    """The result of comparing two Outputs under the law."""

    equal: bool
    stdout_ok: bool
    stderr_ok: bool
    exit_ok: bool
    detail: str = ""


def compare_outputs(
    reference: Outputs,
    candidate: Outputs,
    *,
    mode: str | ComparisonMode = ComparisonMode.EXACT,
    stderr_mode: str = "ignore",
    exit_law: str | ExitLaw = ExitLaw.EXACT,
) -> Verdict:
    """Compare two engine outputs under the single law and return a Verdict.

    This is the one comparison used both for (CPython vs a backend) and for
    (backend vs backend) — the cross-backend divergence sub-oracle reuses it
    verbatim so a backend fork is judged identically to a CPython mismatch.
    """
    if reference.stdout is None or candidate.stdout is None:
        # A missing-output side cannot be compared on stdout; this is a hard
        # mismatch (the build-failure case is resolved by the caller before it
        # reaches here in diff_test).
        return Verdict(
            equal=False,
            stdout_ok=False,
            stderr_ok=False,
            exit_ok=False,
            detail="missing stdout on one side",
        )
    stdout_ok = stdout_matches(reference.stdout, candidate.stdout, mode)
    stderr_ok = stderr_matches(reference.stderr, candidate.stderr, stderr_mode)
    exit_ok = compare_exit(reference.returncode, candidate.returncode, exit_law)
    equal = stdout_ok and stderr_ok and exit_ok
    detail = ""
    if not equal:
        parts: list[str] = []
        if not stdout_ok:
            parts.append("stdout mismatch")
        if not exit_ok:
            parts.append(
                f"exit code ref={reference.returncode} cand={candidate.returncode}"
            )
        if not stderr_ok:
            parts.append("stderr mismatch")
        detail = "; ".join(parts)
    return Verdict(
        equal=equal,
        stdout_ok=stdout_ok,
        stderr_ok=stderr_ok,
        exit_ok=exit_ok,
        detail=detail,
    )


def outputs_equivalent(
    reference: Outputs,
    candidate: Outputs,
    *,
    mode: str | ComparisonMode = ComparisonMode.EXACT,
    stderr_mode: str = "ignore",
    exit_law: str | ExitLaw = ExitLaw.EXACT,
) -> bool:
    """Boolean convenience wrapper over `compare_outputs`."""
    return compare_outputs(
        reference,
        candidate,
        mode=mode,
        stderr_mode=stderr_mode,
        exit_law=exit_law,
    ).equal


def _mode_token(mode: str | ComparisonMode) -> str:
    if isinstance(mode, ComparisonMode):
        return mode.value
    return str(mode).strip().lower()
