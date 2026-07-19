"""Tests for tools/encoding_gate.py -- the encoding-safety ratchet (M43 bug class).

The unit tests drive the pure scanner (`scan_source`) on SYNTHETIC one-line
sources so each rule -- and, just as important, each SAFE variant it must NOT
flag -- is proven deterministically and fast. The ratchet math (`regressions`)
is proven to catch both a brand-new file::rule AND an extra occurrence inside a
file that already trips a rule (the hole a fingerprint set would miss).

The headline falsification ("real gate, not theater", M05): the integration test
drops a throwaway `open("x")` file into the scanned tree and asserts the real
`encoding_gate.py --check` CLI FAILS (exit 2) on it and PASSES (exit 0) on the
clean tree. A gate that only ever passes clean is not proven to have teeth.
"""

from __future__ import annotations
from tests.process_guard_common import run_guarded_test_process

import subprocess
import sys
import uuid
from pathlib import Path

import pytest

REPO_ROOT = Path(__file__).resolve().parents[2]
TOOLS_ROOT = REPO_ROOT / "tools"
if str(TOOLS_ROOT) not in sys.path:
    sys.path.insert(0, str(TOOLS_ROOT))

import encoding_gate as eg  # noqa: E402


# ---------------------------------------------------------------------------
# Rule 1 -- open()/io.open() in text mode without encoding=
# ---------------------------------------------------------------------------


def _rules(source: str) -> list[str]:
    return [v.rule for v in eg.scan_source(source, "synthetic.py")]


@pytest.mark.parametrize(
    "source",
    [
        'open("x")',  # default mode == text
        'open("x", "w")',  # explicit text write
        'open("x", mode="r")',  # keyword text mode
        'io.open("x")',  # io.open alias
        'open("x", encoding=None)',  # encoding=None re-selects the platform default
    ],
)
def test_open_text_without_encoding_is_flagged(source: str) -> None:
    assert _rules(source) == ["open-no-encoding"], source


@pytest.mark.parametrize(
    "source",
    [
        'open("x", encoding="utf-8")',  # pinned encoding
        'open("x", "rb")',  # binary mode -- no text codec involved
        'open("x", "wb")',
        'open("x", mode="rb")',
        'open("x", **kwargs)',  # encoding may be inside kwargs -- conservative
        'open("x", the_mode)',  # non-literal mode -- cannot prove text; stay silent
    ],
)
def test_open_safe_variants_not_flagged(source: str) -> None:
    assert _rules(source) == [], source


# ---------------------------------------------------------------------------
# Rule 2 -- Path.read_text / Path.write_text without encoding=
# ---------------------------------------------------------------------------


def test_read_write_text_without_encoding_flagged() -> None:
    assert _rules("p.read_text()") == ["read_text-no-encoding"]
    assert _rules("p.write_text(data)") == ["write_text-no-encoding"]


def test_read_write_text_with_encoding_clean() -> None:
    assert _rules('p.read_text(encoding="utf-8")') == []
    assert _rules('p.write_text(data, encoding="utf-8")') == []
    assert _rules("p.write_text(data, **kw)") == []  # conservative on **kwargs


# ---------------------------------------------------------------------------
# Rule 3 -- subprocess text mode without encoding=
# ---------------------------------------------------------------------------


@pytest.mark.parametrize(
    "source",
    [
        "subprocess.run(cmd, text=True)",
        "subprocess.Popen(cmd, universal_newlines=True)",
        "subprocess.check_output(cmd, text=True)",
        "run(cmd, text=True)",  # bare `from subprocess import run`
    ],
)
def test_subprocess_text_without_encoding_flagged(source: str) -> None:
    assert _rules(source) == ["subprocess-text-no-encoding"], source


@pytest.mark.parametrize(
    "source",
    [
        'subprocess.run(cmd, text=True, encoding="utf-8")',  # pinned
        "subprocess.run(cmd)",  # bytes mode -- no decode of child output
        "subprocess.run(cmd, text=False)",  # explicitly bytes
        "subprocess.run(cmd, text=want_text)",  # non-literal -- cannot prove text
        "subprocess.run(cmd, **kw)",  # conservative on **kwargs
    ],
)
def test_subprocess_safe_variants_not_flagged(source: str) -> None:
    assert _rules(source) == [], source


# ---------------------------------------------------------------------------
# Ratchet math -- regressions() catches NEW keys AND +1 within an existing key
# ---------------------------------------------------------------------------


def test_regressions_flags_new_key() -> None:
    base = {"a.py::open-no-encoding": 1}
    counts = {"a.py::open-no-encoding": 1, "b.py::read_text-no-encoding": 1}
    assert eg.regressions(counts, base) == ["b.py::read_text-no-encoding"]


def test_regressions_flags_extra_occurrence_in_existing_key() -> None:
    # The hole a fingerprint SET would miss: same file::rule, one more site.
    base = {"a.py::read_text-no-encoding": 3}
    counts = {"a.py::read_text-no-encoding": 4}
    assert eg.regressions(counts, base) == ["a.py::read_text-no-encoding"]


def test_regressions_allows_burn_down() -> None:
    base = {"a.py::read_text-no-encoding": 3, "b.py::open-no-encoding": 1}
    counts = {"a.py::read_text-no-encoding": 1}  # fixed some, removed a file entirely
    assert eg.regressions(counts, base) == []


# ---------------------------------------------------------------------------
# Integration -- the real CLI has teeth (fails on a planted violation)
# ---------------------------------------------------------------------------


def _run_check() -> subprocess.CompletedProcess[str]:
    return run_guarded_test_process(
        [sys.executable, str(TOOLS_ROOT / "encoding_gate.py"), "--check"],
        cwd=str(REPO_ROOT),
        capture_output=True,
        text=True,
        encoding="utf-8",
        errors="replace",
        timeout=120,
    )


def test_clean_tree_passes() -> None:
    result = _run_check()
    assert result.returncode == 0, f"expected clean PASS, got:\n{result.stderr}"
    assert "PASS" in result.stdout


def test_planted_violation_fails() -> None:
    """Drop a throwaway `open("x")` into the scanned tree; --check MUST fail on it."""
    planted = TOOLS_ROOT / f"_encoding_gate_teeth_{uuid.uuid4().hex}.py"
    planted.write_text('open("x")\n', encoding="utf-8")
    try:
        result = _run_check()
    finally:
        planted.unlink(missing_ok=True)
    assert result.returncode == 2, (
        f"planted violation did not trip the gate (rc={result.returncode}); "
        f"the gate lacks teeth.\nstdout:\n{result.stdout}\nstderr:\n{result.stderr}"
    )
    assert planted.name in result.stderr
    assert "open-no-encoding" in result.stderr

    # And, crucially, removing the plant returns the tree to green.
    assert _run_check().returncode == 0
