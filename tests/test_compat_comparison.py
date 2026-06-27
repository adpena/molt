"""Unit tests for the single CPython-parity comparison law (doc 66 Phase 0).

tools/compat/comparison.py is the ONE comparison law in the tree after the
extraction. These tests:
  1. Pin each axis of the law (stdout canonicalization incl. pyperformance +
     relaxed, the exception-signature stderr law, the exit-code laws).
  2. Prove the extracted law is BYTE-IDENTICAL to the implementation it replaced
     in tests/molt_diff.py — the acceptance gate from doc 66 §6 Risk 1: "the law
     moved, the results must not." We re-derive the original inline behavior here
     and assert the new law agrees over a broad input matrix.
  3. Exercise the composite verdict used both for (CPython vs backend) and the
     cross-backend divergence sub-oracle.
"""

from __future__ import annotations

import re
import sys
from pathlib import Path

import pytest

_REPO_ROOT = Path(__file__).resolve().parents[1]
_TOOLS = _REPO_ROOT / "tools"
if str(_TOOLS) not in sys.path:
    sys.path.insert(0, str(_TOOLS))

from compat import comparison as c  # noqa: E402


# ---------------------------------------------------------------------------
# The ORIGINAL inline implementations (verbatim from the pre-extraction
# tests/molt_diff.py) — the oracle this extraction must reproduce byte-for-byte.
# ---------------------------------------------------------------------------

_ORIG_NUM_RE = re.compile(
    r"(?<![A-Za-z0-9_])[-+]?(?:\d+(?:\.\d+)?|\.\d+)(?:[eE][-+]?\d+)?(?![A-Za-z0-9_])"
)
_ORIG_SPACE_RE = re.compile(r"\s+")
_ORIG_SIG_RE = re.compile(r"^(?P<etype>[A-Za-z_][A-Za-z0-9_.]*)(?:: (?P<message>.*))?$")


def _orig_canonicalize_stdout(text: str, mode: str) -> str:
    normalized = mode.strip().lower()
    if normalized in {"", "exact"}:
        return text
    if normalized == "pyperformance":
        lines: list[str] = []
        for raw_line in text.splitlines():
            line = raw_line.strip()
            if not line:
                continue
            line = _ORIG_NUM_RE.sub("<num>", line)
            line = _ORIG_SPACE_RE.sub(" ", line)
            lines.append(line)
        return "\n".join(lines)
    return text


def _orig_extract_exception_signature(stderr: str):
    lines = [line.strip() for line in stderr.splitlines() if line.strip()]
    for line in reversed(lines):
        match = _ORIG_SIG_RE.match(line)
        if match is None:
            continue
        etype = match.group("etype")
        message = match.group("message") or ""
        return etype, message
    return None


def _orig_stderr_matches(cpython_stderr: str, molt_stderr: str, mode: str) -> bool:
    normalized = mode.strip().lower()
    if normalized in {"", "ignore"}:
        return True
    if normalized in {"match", "exact"}:
        return cpython_stderr == molt_stderr
    if normalized in {"traceback", "exception", "exception_signature"}:
        cpython_sig = _orig_extract_exception_signature(cpython_stderr)
        molt_sig = _orig_extract_exception_signature(molt_stderr)
        if cpython_sig is None or molt_sig is None:
            return cpython_stderr == molt_stderr
        return cpython_sig == molt_sig
    return cpython_stderr == molt_stderr


# The parity_gate.py RELAXED normalizer (verbatim) — folded into the one law as
# the RELAXED mode; this is its acceptance oracle.
_ORIG_ADDR_RE = re.compile(r"\b0x[0-9a-fA-F]+\b")
_ORIG_REFCOUNT_RE = re.compile(r"\brefcount\s*=\s*\d+", re.IGNORECASE)
_ORIG_OBJ_ADDR_RE = re.compile(r"(at|id=)\s*0x[0-9a-fA-F]+")


def _orig_normalize_relaxed(text: str) -> str:
    text = _ORIG_ADDR_RE.sub("0xADDR", text)
    text = _ORIG_OBJ_ADDR_RE.sub(r"\g<1> 0xADDR", text)
    text = _ORIG_REFCOUNT_RE.sub("refcount=<N>", text)
    return text


# ---------------------------------------------------------------------------
# Input corpora
# ---------------------------------------------------------------------------

_STDOUT_SAMPLES = [
    "",
    "3\n",
    "hello world\n",
    "1\n2\n3\n",
    "value = 3.14159\n",
    "x = -10  y =  20\n",
    "result: 1.5e10 and 0.0001\n",
    "  leading and trailing  \n\n",
    "no_numbers_here\njust_text\n",
    "mix 1 of 2.0 and -3 with 4e5\n",
    "tab\tseparated\tvalues\n",
    "list = [1, 2, 3]\ntuple = (4, 5)\n",
    "float128 versus float16\n",  # alnum-adjacent digits must NOT collapse
]

_STDERR_SAMPLES = [
    "",
    "ValueError: bad value\n",
    "Traceback (most recent call last):\n  File x\nValueError: bad value\n",
    "Traceback (most recent call last):\n  File y\nValueError: bad value\n",
    "KeyError: 'missing'\n",
    "ZeroDivisionError: division by zero\n",
    "some warning line\nTypeError: unsupported operand\n",
    "RuntimeError\n",
    "molt.foo.BarError: detail here\n",
]

_RELAXED_SAMPLES = [
    "<object at 0x7fa1b2c3>\n",
    "id=0xDEADBEEF refcount=5\n",
    "<Foo at 0x10> and <Bar at 0x20>\n",
    "refcount = 3\n",
    "plain text no addresses\n",
    "0xabc 0x123 0xFFFF\n",
]


# ---------------------------------------------------------------------------
# Byte-for-byte equivalence (the law moved; results must not)
# ---------------------------------------------------------------------------


@pytest.mark.parametrize("text", _STDOUT_SAMPLES)
@pytest.mark.parametrize("mode", ["", "exact", "pyperformance", "PYPERFORMANCE"])
def test_canonicalize_stdout_matches_original(text: str, mode: str) -> None:
    assert c.canonicalize_stdout(text, mode) == _orig_canonicalize_stdout(text, mode)


@pytest.mark.parametrize("cp", _STDERR_SAMPLES)
@pytest.mark.parametrize("molt", _STDERR_SAMPLES)
@pytest.mark.parametrize(
    "mode", ["", "ignore", "match", "exact", "exception_signature", "traceback"]
)
def test_stderr_matches_matches_original(cp: str, molt: str, mode: str) -> None:
    assert c.stderr_matches(cp, molt, mode) == _orig_stderr_matches(cp, molt, mode)


@pytest.mark.parametrize("text", _STDERR_SAMPLES)
def test_extract_exception_signature_matches_original(text: str) -> None:
    assert c.extract_exception_signature(text) == _orig_extract_exception_signature(
        text
    )


@pytest.mark.parametrize("text", _RELAXED_SAMPLES)
def test_normalize_relaxed_matches_parity_gate(text: str) -> None:
    assert c.normalize_relaxed(text) == _orig_normalize_relaxed(text)
    # And RELAXED is reachable as a canonicalization mode.
    assert c.canonicalize_stdout(text, "relaxed") == _orig_normalize_relaxed(text)


# ---------------------------------------------------------------------------
# Axis behavior pins
# ---------------------------------------------------------------------------


def test_pyperformance_collapses_numbers_but_not_alnum_adjacent() -> None:
    assert c.canonicalize_stdout("x = 3.14\n", "pyperformance") == "x = <num>"
    # Digits adjacent to letters are NOT numeric tokens (float16 stays float16).
    assert c.canonicalize_stdout("float16\n", "pyperformance") == "float16"


def test_exception_signature_ignores_frames_keeps_type_and_message() -> None:
    cp = "Traceback (most recent call last):\n  File a\nValueError: x\n"
    molt = "Traceback (oh no):\n  at wasm frame 7\nValueError: x\n"
    assert c.stderr_matches(cp, molt, "exception_signature") is True
    # Different message -> mismatch.
    molt2 = "Traceback\nValueError: y\n"
    assert c.stderr_matches(cp, molt2, "exception_signature") is False
    # Different type -> mismatch.
    molt3 = "Traceback\nKeyError: x\n"
    assert c.stderr_matches(cp, molt3, "exception_signature") is False


def test_exit_law_exact_vs_compatible() -> None:
    assert c.compare_exit(0, 0, c.ExitLaw.EXACT) is True
    assert c.compare_exit(1, 1, c.ExitLaw.EXACT) is True
    assert c.compare_exit(1, 2, c.ExitLaw.EXACT) is False
    # Compatible: only the success/failure partition must agree.
    assert c.compare_exit(1, 2, c.ExitLaw.COMPATIBLE) is True
    assert c.compare_exit(0, 0, c.ExitLaw.COMPATIBLE) is True
    assert c.compare_exit(0, 1, c.ExitLaw.COMPATIBLE) is False
    assert c.compare_exit(1, 0, c.ExitLaw.COMPATIBLE) is False
    # String law tokens accepted too.
    assert c.compare_exit(3, 9, "compatible") is True
    assert c.compare_exit(3, 9, "exact") is False


# ---------------------------------------------------------------------------
# Composite verdict / cross-backend usage
# ---------------------------------------------------------------------------


def test_compare_outputs_equal_when_all_axes_agree() -> None:
    ref = c.Outputs("42\n", "", 0)
    cand = c.Outputs("42\n", "", 0)
    verdict = c.compare_outputs(ref, cand)
    assert verdict.equal is True
    assert verdict.detail == ""


def test_compare_outputs_flags_stdout_mismatch() -> None:
    ref = c.Outputs("42\n", "", 0)
    cand = c.Outputs("43\n", "", 0)
    verdict = c.compare_outputs(ref, cand)
    assert verdict.equal is False
    assert verdict.stdout_ok is False
    assert "stdout mismatch" in verdict.detail


def test_compare_outputs_flags_exit_mismatch() -> None:
    ref = c.Outputs("x\n", "", 0)
    cand = c.Outputs("x\n", "", 3)
    verdict = c.compare_outputs(ref, cand)
    assert verdict.equal is False
    assert verdict.exit_ok is False
    assert "exit code" in verdict.detail


def test_compare_outputs_missing_stdout_is_mismatch() -> None:
    ref = c.Outputs("x\n", "", 0)
    cand = c.Outputs(None, "build failed", 1)
    verdict = c.compare_outputs(ref, cand)
    assert verdict.equal is False


def test_compare_outputs_used_for_cross_backend_is_symmetric_in_meaning() -> None:
    # Two backends that agree -> equal; the divergence sub-oracle uses this rule.
    a = c.Outputs("ok\n", "", 0)
    b = c.Outputs("ok\n", "", 0)
    assert c.outputs_equivalent(a, b) is True
    # One backend diverges -> not equal (a FAIL in the oracle).
    bad = c.Outputs("ok\n<FAULT>\n", "", 0)
    assert c.outputs_equivalent(a, bad) is False


def test_pyperformance_mode_applies_in_compare_outputs() -> None:
    # Same shape, different numbers -> equal under pyperformance.
    ref = c.Outputs("time: 1.23\n", "", 0)
    cand = c.Outputs("time: 4.56\n", "", 0)
    assert c.outputs_equivalent(ref, cand, mode="pyperformance") is True
    # But NOT equal under exact.
    assert c.outputs_equivalent(ref, cand, mode="exact") is False
