"""Tests for tools/encoding_gate.py -- the encoding-safety ratchet (M43 bug class).

The unit tests drive the pure scanner (`scan_source`) on SYNTHETIC one-line
sources so each rule -- and, just as important, each SAFE variant it must NOT
flag -- is proven deterministically and fast. The ratchet math (`regressions`)
is proven to catch both a brand-new file::rule AND an extra occurrence inside a
file that already trips a rule (the hole a fingerprint set would miss).

The headline falsification ("real gate, not theater", M05): the integration test
drops a throwaway `open("x")` file into a fixture checkout and asserts the real
`encoding_gate.py --check --root` CLI FAILS (exit 2) on it and PASSES (exit 0)
once it is gone. A gate that only ever passes clean is not proven to have teeth.
The plant never enters this repository: a file there would dirty the checkout
that every parallel proof command attests.
"""

from __future__ import annotations
from tests.process_guard_common import run_guarded_test_process

import subprocess
import sys
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


def _run_check(root: Path = REPO_ROOT) -> subprocess.CompletedProcess[str]:
    return run_guarded_test_process(
        [
            sys.executable,
            str(TOOLS_ROOT / "encoding_gate.py"),
            "--check",
            "--root",
            str(root),
        ],
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


def test_planted_violation_fails(tmp_path: Path) -> None:
    """Plant a throwaway `open("x")` in a scanned checkout; --check MUST fail on it."""
    root = tmp_path / "checkout"
    (root / "tools").mkdir(parents=True)
    (root / "tools" / "encoding_gate_baseline.json").write_bytes(
        (TOOLS_ROOT / "encoding_gate_baseline.json").read_bytes()
    )
    assert _run_check(root).returncode == 0

    planted = root / "tools" / "planted.py"
    planted.write_text('open("x")\n', encoding="utf-8")
    result = _run_check(root)
    assert result.returncode == 2, (
        f"planted violation did not trip the gate (rc={result.returncode}); "
        f"the gate lacks teeth.\nstdout:\n{result.stdout}\nstderr:\n{result.stderr}"
    )
    assert "tools/planted.py" in result.stderr
    assert "open-no-encoding" in result.stderr

    # And, crucially, removing the plant returns the checkout to green.
    planted.unlink()
    assert _run_check(root).returncode == 0


def test_fix_pins_utf8_on_every_flagged_call_and_stays_valid() -> None:
    source = (
        "import subprocess\n"
        "from pathlib import Path\n"
        "p = Path('x')\n"
        "p.read_text()\n"
        "p.write_text('data',\n"
        "             errors='strict')\n"
        "open('f', encoding=None)\n"
        "open('f', 'rb')\n"
        "subprocess.run(['x'], text=True)\n"
        "label = '\u2028 not a line break'\n"
        "p.read_text(**kwargs)\n"
    )
    fixed, count = eg.fix_source(source, "tools/fixture.py")

    assert count == 4
    assert eg.scan_source(fixed, "tools/fixture.py") == []
    assert 'p.read_text(encoding="utf-8")' in fixed
    assert "errors='strict', encoding=\"utf-8\")" in fixed
    assert "open('f', encoding=\"utf-8\")" in fixed
    assert "open('f', 'rb')" in fixed  # binary mode is never touched
    assert "p.read_text(**kwargs)" in fixed  # forwarded kwargs may carry it
    assert eg.fix_source(fixed, "tools/fixture.py") == (fixed, 0)


def test_positional_encodings_and_foreign_read_text_are_not_violations() -> None:
    source = (
        "from importlib import metadata\n"
        "from pathlib import Path\n"
        "p = Path('x')\n"
        "p.read_text('utf-8')\n"
        "p.write_text('data', 'utf-8')\n"
        "open('f', 'r', -1, 'utf-8')\n"
        "next(metadata.distributions()).read_text('direct_url.json')\n"
    )
    assert eg.scan_source(source, "tools/fixture.py") == []
    assert eg.fix_source(source, "tools/fixture.py") == (source, 0)


def test_identifier_prefilter_never_hides_a_violation() -> None:
    # Non-ASCII source can spell an identifier through NFKC normalization, so
    # the prefilter must still parse it; ASCII source without a checked name or
    # without a text-mode keyword has no violation to find.
    assert _rules('ｏｐｅｎ("p")\n') == ["open-no-encoding"]
    assert _rules('subprocess.run(["x"], text=True)\n') == [
        "subprocess-text-no-encoding"
    ]
    assert eg._may_violate("x = 1\n") is False
    assert eg._may_violate('subprocess.run(["x"])\n') is False
