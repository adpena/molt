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

import os
import subprocess
import sys
from pathlib import Path

import pytest

REPO_ROOT = Path(__file__).resolve().parents[2]
TOOLS_ROOT = REPO_ROOT / "tools"


from tools import encoding_gate as eg  # noqa: E402
from tools import scan_records as sr  # noqa: E402


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


def test_check_reports_all_current_locations_in_regressed_groups(
    tmp_path: Path, capsys: pytest.CaptureFixture[str]
) -> None:
    baseline = tmp_path / "tools" / "encoding_gate_baseline.json"
    baseline.parent.mkdir()
    baseline_bytes = (
        b'{"total": 2, "by_site": {"tools/report.py::open-no-encoding": 1, '
        b'"tools/report.py::write_text-no-encoding": 1}}\n'
    )
    baseline.write_bytes(baseline_bytes)
    source = (
        'open("first")\n'
        "p.read_text()\n"
        'open("second")\n'
        "p.read_text()\n"
        'p.write_text("existing")\n'
    )
    expected = (
        "encoding_gate: FAIL -- 3 excess encoding-unsafe call site(s) in 2 "
        "file/rule group(s) above baseline; showing all 4 current call site(s):\n"
        "    tools/report.py::open-no-encoding  (1->2, +1)\n"
        "        tools/report.py:1: open-no-encoding\n"
        "        tools/report.py:3: open-no-encoding\n"
        "    tools/report.py::read_text-no-encoding  (NEW; 0->2, +2)\n"
        "        tools/report.py:2: read_text-no-encoding\n"
        "        tools/report.py:4: read_text-no-encoding\n"
    )
    outputs = []
    for _ in range(2):
        assert eg._cmd_check(eg.scan_source(source, "tools/report.py"), tmp_path) == 2
        captured = capsys.readouterr()
        assert captured.out == ""
        assert captured.err.startswith(expected)
        assert "count-only baseline cannot identify which individual sites are new" in (
            captured.err
        )
        assert "write_text-no-encoding" not in captured.err
        outputs.append(captured.err)
    assert outputs[0] == outputs[1]
    assert baseline.read_bytes() == baseline_bytes


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


# ---------------------------------------------------------------------------
# Scan records -- tools/scan_records.py, the shared per-file result authority.
# The encoding gate is its first consumer, so its contract is proven here
# through the real gate scan; the other consumers prove their own codecs.
# ---------------------------------------------------------------------------

_OWNER = TOOLS_ROOT / "encoding_gate.py"


def _records(cache: Path, owner: Path = _OWNER) -> sr.ScanRecords:
    return sr.ScanRecords("encoding_gate", owner=owner, cache_root=cache)


def _sites(violations: list[eg.Violation]) -> list[tuple[str, int, str]]:
    return [(v.relpath, v.line, v.rule) for v in violations]


def _encoding_checkout(root: Path) -> Path:
    tools = root / "tools"
    tools.mkdir(parents=True)
    (tools / "two.py").write_text('open("a")\np.read_text()\n', encoding="utf-8")
    (tools / "clean.py").write_text('open("a", "rb")\n', encoding="utf-8")
    (tools / "crlf.py").write_bytes(b'x = 1\r\nsubprocess.run(["x"], text=True)\r\n')
    # Undecodable bytes take the gate's errors="replace" path.
    (tools / "bytes.py").write_bytes(b'"\xff"\nopen("a", mode="w")\n')
    (tools / "café.py").write_text('p.write_text("x")\n', encoding="utf-8")
    return root


def test_scan_records_replay_exactly_what_the_scan_computes(tmp_path: Path) -> None:
    root = _encoding_checkout(tmp_path / "checkout")
    cache = tmp_path / "cache"
    expected = _sites(eg.scan(root))
    assert len(expected) == 5

    cold = _records(cache)
    assert _sites(eg.scan(root, cold)) == expected
    assert (cold.hits, cold.misses) == (0, 5)
    assert cold.publish() is True

    warm = _records(cache)
    assert _sites(eg.scan(root, warm)) == expected
    assert (warm.hits, warm.misses) == (5, 0)
    # A fully replayed run writes nothing.
    assert warm.publish() is False


def test_scan_records_catch_a_seeded_violation_cold_and_warm(tmp_path: Path) -> None:
    root = _encoding_checkout(tmp_path / "checkout")
    cache = tmp_path / "cache"
    clean = root / "tools" / "clean.py"
    before = _sites(eg.scan(root))
    for _ in range(2):
        records = _records(cache)
        assert _sites(eg.scan(root, records)) == before
        records.publish()

    clean.write_text('open("a", "rb")\nopen("seeded")\n', encoding="utf-8")
    seeded = _records(cache)
    found = _sites(eg.scan(root, seeded))
    assert ("tools/clean.py", 2, "open-no-encoding") in found
    assert (seeded.hits, seeded.misses) == (4, 1)
    seeded.publish()
    warm = _records(cache)
    assert _sites(eg.scan(root, warm)) == found
    assert warm.misses == 0

    # Restoring the bytes replays the original record.
    clean.write_text('open("a", "rb")\n', encoding="utf-8")
    restored = _records(cache)
    assert _sites(eg.scan(root, restored)) == before
    assert restored.misses == 0


def test_scan_records_are_bound_to_the_scanner_source(tmp_path: Path) -> None:
    root = _encoding_checkout(tmp_path / "checkout")
    cache = tmp_path / "cache"
    owner = tmp_path / "scanner.py"
    owner.write_bytes(_OWNER.read_bytes())
    first = _records(cache, owner)
    eg.scan(root, first)
    assert first.publish() is True
    assert _records(cache, owner).generation == first.generation

    owner.write_bytes(_OWNER.read_bytes() + b"# edited\n")
    edited = _records(cache, owner)
    assert edited.generation != first.generation
    eg.scan(root, edited)
    assert (edited.hits, edited.misses) == (0, 5)


@pytest.mark.parametrize(
    "payload",
    [b"", b"{not json", b"[]", b'{"schema": 999, "records": {}}'],
)
def test_scan_records_treat_an_unusable_file_as_empty(
    tmp_path: Path, payload: bytes
) -> None:
    root = _encoding_checkout(tmp_path / "checkout")
    cache = tmp_path / "cache"
    probe = _records(cache)
    assert probe.path is not None
    probe.path.parent.mkdir(parents=True)
    probe.path.write_bytes(payload)

    records = _records(cache)
    expected = _sites(eg.scan(root))
    assert _sites(eg.scan(root, records)) == expected
    assert records.misses == 5
    assert records.publish() is True
    assert _records(cache)._load().keys() == records._used.keys()


def test_scan_records_publish_nothing_after_a_failed_scan(tmp_path: Path) -> None:
    cache = tmp_path / "cache"

    def failing() -> list[eg.Violation]:
        raise UnicodeDecodeError("utf-8", b"\xff", 0, 1, "fixture")

    with pytest.raises(UnicodeDecodeError):
        with sr.open_scan_records(
            "encoding_gate", owner=_OWNER, enabled=True, cache_root=cache
        ) as records:
            assert records is not None
            records.result("tools/ok.py", b"x = 1\n", list, encode=list, decode=list)
            records.result("tools/bad.py", b"\xff", failing, encode=list, decode=list)
    assert not (cache / "repository-scan-records").exists()


def test_scan_records_refuse_publication_after_new_first_party_code(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    root = _encoding_checkout(tmp_path / "checkout")
    records = _records(tmp_path / "cache")
    eg.scan(root, records)
    # A first-party module imported during the run could have shaped results.
    late = tmp_path / "late_module.py"
    late.write_text("VALUE = 1\n", encoding="utf-8")
    module = type(sys)("tools._scan_records_late_fixture")
    module.__file__ = str(late)
    monkeypatch.setitem(sys.modules, module.__name__, module)
    assert records.publish() is False
    assert records.path is not None and not records.path.exists()


def test_scan_records_from_a_narrow_run_keep_the_other_records(tmp_path: Path) -> None:
    root = _encoding_checkout(tmp_path / "checkout")
    cache = tmp_path / "cache"
    full = _records(cache)
    eg.scan(root, full)
    full.publish()

    narrow_root = tmp_path / "narrow"
    (narrow_root / "tools").mkdir(parents=True)
    (narrow_root / "tools" / "one.py").write_text('open("n")\n', encoding="utf-8")
    narrow = _records(cache)
    eg.scan(narrow_root, narrow)
    assert narrow.publish() is True

    warm = _records(cache)
    eg.scan(root, warm)
    assert (warm.hits, warm.misses) == (5, 0)


def test_scan_records_renew_used_records_and_expire_unused_ones(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    root = _encoding_checkout(tmp_path / "checkout")
    cache = tmp_path / "cache"
    clock = {"now": 1_000_000_000}
    monkeypatch.setattr(sr, "_wall_clock", lambda: clock["now"])
    first = _records(cache)
    eg.scan(root, first)
    first.publish()

    clock["now"] += sr.RECORD_RENEWAL_SECONDS - 1
    fresh = _records(cache)
    eg.scan(root, fresh)
    assert fresh.misses == 0 and fresh.publish() is False

    clock["now"] += 2
    stale = _records(cache)
    eg.scan(root, stale)
    assert stale.misses == 0 and stale.publish() is True

    # Only one file stays in use; the other four expire after their lifetime.
    only = tmp_path / "only"
    (only / "tools").mkdir(parents=True)
    (only / "tools" / "two.py").write_bytes((root / "tools" / "two.py").read_bytes())
    clock["now"] += sr.RECORD_LIFETIME_SECONDS - sr.RECORD_RENEWAL_SECONDS
    narrow = _records(cache)
    eg.scan(only, narrow)
    narrow.publish()
    assert len(_records(cache)._load()) == 5
    clock["now"] += sr.RECORD_RENEWAL_SECONDS
    narrow = _records(cache)
    eg.scan(only, narrow)
    assert narrow.publish() is True
    assert list(_records(cache)._load()) == list(narrow._used)


def test_scan_records_retire_old_generations_and_interrupted_stages(
    tmp_path: Path,
) -> None:
    cache = tmp_path / "cache"
    records = _records(cache)
    directory = records.directory
    directory.mkdir(parents=True)
    now = records._now
    older = []
    for age in range(1, 6):
        path = directory / f"{age:064x}.json"
        path.write_bytes(b"{}")
        os.utime(path, (now - age * 60, now - age * 60))
        older.append(path)
    stale_stage = directory / f".molt-write-{'0' * 16}-{'1' * 32}.tmp"
    fresh_stage = directory / f".molt-write-{'0' * 16}-{'2' * 32}.tmp"
    for stage, age in ((stale_stage, sr.STALE_STAGE_SECONDS + 60), (fresh_stage, 60)):
        stage.write_bytes(b"")
        os.utime(stage, (now - age, now - age))

    records.result("tools/a.py", b"x = 1\n", list, encode=list, decode=list)
    assert records.publish() is True
    remaining = {path.name for path in directory.iterdir()}
    assert records.path is not None
    kept = {records.path.name, *(path.name for path in older[:3])}
    assert remaining == kept | {fresh_stage.name}


def test_library_entry_points_open_no_records(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    monkeypatch.setenv("MOLT_CACHE", str(tmp_path / "cache"))
    root = _encoding_checkout(tmp_path / "checkout")
    assert eg.main(["--list", "--root", str(root)]) == 0
    assert not (tmp_path / "cache").exists()


@pytest.mark.parametrize(
    ("data", "encoding", "errors"),
    [
        (b"a\r\nb\rc\n", "utf-8", "strict"),
        (b"\xef\xbb\xbfx = 1\r\n", "utf-8-sig", "strict"),
        (b"ok\xff\xfe\r\n", "utf-8", "replace"),
        (b"\r", "utf-8", "strict"),
    ],
)
def test_scan_records_decode_exactly_as_path_read_text(
    tmp_path: Path, data: bytes, encoding: str, errors: str
) -> None:
    path = tmp_path / "source.py"
    path.write_bytes(data)
    assert sr.decoded_text(data, encoding=encoding, errors=errors) == path.read_text(
        encoding=encoding, errors=errors
    )


def test_cli_replays_records_and_still_fails_a_planted_violation(
    tmp_path: Path,
) -> None:
    cache = tmp_path / "cache"
    env = {**os.environ, "MOLT_CACHE": str(cache)}
    root = tmp_path / "checkout"
    (root / "tools").mkdir(parents=True)
    (root / "tools" / "encoding_gate_baseline.json").write_bytes(
        (TOOLS_ROOT / "encoding_gate_baseline.json").read_bytes()
    )
    (root / "tools" / "clean.py").write_text('open("a", "rb")\n', encoding="utf-8")

    def check() -> subprocess.CompletedProcess[str]:
        return run_guarded_test_process(
            [sys.executable, str(_OWNER), "--check", "--root", str(root)],
            cwd=str(REPO_ROOT),
            env=env,
            capture_output=True,
            text=True,
            encoding="utf-8",
            errors="replace",
            timeout=120,
        )

    assert check().returncode == 0
    published = list((cache / "repository-scan-records" / "encoding_gate").iterdir())
    assert len(published) == 1
    assert check().returncode == 0  # replayed

    planted = root / "tools" / "planted.py"
    planted.write_text('open("x")\n', encoding="utf-8")
    result = check()
    assert result.returncode == 2, result.stdout + result.stderr
    assert "tools/planted.py" in result.stderr
    assert check().returncode == 2  # the replayed violation still fails

    planted.unlink()
    assert check().returncode == 0
