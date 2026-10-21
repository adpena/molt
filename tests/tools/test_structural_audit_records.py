"""Scan records in tools/structural_audit.py replay exactly what the probes compute.

The per-file probes (debt markers, Python and Rust stub surfaces and their
compatibility inventories, process-wide test patches, build-failure skips and
scratch-authority sites) route their pure per-file step through
``tools/scan_records.py``. The oracle is the computed run: a replayed run must
return the same findings, a seeded violation must be caught whether its file
replays or computes, and restoring the bytes replays the earlier record.
"""

from __future__ import annotations

import os
import sys
from pathlib import Path

import pytest

from tests.process_guard_common import run_guarded_test_process
from tools import scan_records
from tools import structural_audit as SA

ROOT = Path(__file__).resolve().parents[2]
AUDIT = ROOT / "tools" / "structural_audit.py"
# The per-file probes that consult scan records.
RECORD_PROBES = {
    "debt_marker",
    "python_stub_surface",
    "rust_stub_surface",
    "process_wide_test_patch",
    "build_failure_test_skip",
    "scratch_authority_bypass",
    "compatibility_error_applicability",
    "compatibility_error_outcome",
}


def _records(cache: Path) -> scan_records.ScanRecords:
    return scan_records.ScanRecords("structural_audit", owner=AUDIT, cache_root=cache)


def _write(root: Path, rel: str, text: str) -> Path:
    path = root / rel
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(text, encoding="utf-8")
    return path


def _checkout(root: Path) -> Path:
    _write(
        root,
        "src/molt/feature.py",
        "# TODO(compiler): route through generated facts\n"
        "def stub():\n    raise NotImplementedError\n",
    )
    _write(
        root,
        "runtime/molt-runtime/src/lib.rs",
        'pub fn missing() { todo!("real implementation"); }\n'
        "pub fn other() { unimplemented!() }\n",
    )
    _write(
        root,
        "tests/test_patches.py",
        "import pytest\n"
        "def test_a(monkeypatch):\n"
        "    monkeypatch.setattr(mod.os, 'getpid', lambda: 1)\n"
        "def test_b():\n"
        "    pytest.skip('build failed: linker exited')\n",
    )
    _write(
        root,
        "tools/scratch_user.py",
        "from pathlib import Path\nSCRATCH = Path('repo') / 'tmp'\n",
    )
    return root


def _record_findings(findings: list[SA.Finding]) -> list[SA.Finding]:
    return [finding for finding in findings if finding.probe in RECORD_PROBES]


def test_records_replay_the_computed_findings_of_a_checkout(tmp_path: Path) -> None:
    root = _checkout(tmp_path / "checkout")
    computed = SA.run_all(root)
    assert {finding.probe for finding in _record_findings(computed)} >= {
        "debt_marker",
        "python_stub_surface",
        "rust_stub_surface",
        "process_wide_test_patch",
        "build_failure_test_skip",
        "scratch_authority_bypass",
    }
    cold = _records(tmp_path / "cache")
    assert SA.run_all(root, records=cold) == computed
    assert cold.misses > 0 and cold.publish() is True
    warm = _records(tmp_path / "cache")
    assert SA.run_all(root, records=warm) == computed
    assert (warm.hits, warm.misses) == (cold.misses + cold.hits, 0)


def test_records_catch_seeded_violations_cold_and_warm(tmp_path: Path) -> None:
    root = _checkout(tmp_path / "checkout")
    cache = tmp_path / "cache"
    before = SA.run_all(root)
    for _ in range(2):
        records = _records(cache)
        assert SA.run_all(root, records=records) == before
        records.publish()

    feature = root / "src/molt/feature.py"
    original = feature.read_bytes()
    feature.write_bytes(original + b"# HACK: seeded\n")
    tests = root / "tests/test_patches.py"
    tests_original = tests.read_bytes()
    tests.write_bytes(
        tests_original + b"def test_c(monkeypatch):\n"
        b"    monkeypatch.setattr(mod.subprocess, 'run', fake)\n"
    )
    for expected_misses in (True, False):
        records = _records(cache)
        seeded = SA.run_all(root, records=records)
        debt = {f.location: f.metric for f in seeded if f.probe == "debt_marker"}
        patches = [f for f in seeded if f.probe == "process_wide_test_patch"]
        assert debt["src/molt/feature.py:1"] == 2
        assert [finding.metric for finding in patches] == [2]
        assert (records.misses > 0) is expected_misses
        records.publish()

    feature.write_bytes(original)
    tests.write_bytes(tests_original)
    restored = _records(cache)
    assert SA.run_all(root, records=restored) == before
    assert restored.misses == 0


def test_scratch_records_keep_test_modules_apart(tmp_path: Path) -> None:
    # The same bytes are a violation in producer code and a fixture in a test.
    root = tmp_path / "checkout"
    text = "from pathlib import Path\nSCRATCH = Path('anything') / 'tmp'\n"
    _write(root, "tools/producer.py", text)
    _write(root, "tests/test_consumer.py", text)
    cache = tmp_path / "cache"
    for _ in range(2):
        records = _records(cache)
        with SA.audit_operation(root, None, records):
            found = SA.probe_scratch_authority_bypasses(root)
        assert [finding.location.split(":")[0] for finding in found] == [
            "tools/producer.py"
        ]
        records.publish()


def test_command_replays_the_real_tree_audit(tmp_path: Path) -> None:
    env = {**os.environ, "MOLT_CACHE": str(tmp_path / "cache")}

    def audit() -> str:
        return run_guarded_test_process(
            [sys.executable, str(AUDIT), "--json"],
            cwd=ROOT,
            env=env,
            check=True,
            timeout=300,
        ).stdout

    computed = audit()
    published = tmp_path / "cache" / "repository-scan-records" / "structural_audit"
    assert len(list(published.iterdir())) == 1
    assert audit() == computed


class _Stop(Exception):
    pass


@pytest.mark.parametrize("receipt", [False, True])
def test_only_the_command_without_a_release_receipt_opens_records(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch, receipt: bool
) -> None:
    opened = []

    def run_all(root, path_scope=None, records=None):
        opened.append(records)
        raise _Stop

    monkeypatch.setenv("MOLT_CACHE", str(tmp_path / "cache"))
    monkeypatch.setattr(SA, "run_all", run_all)
    monkeypatch.setattr(
        SA.release_receipt,
        "prepare_receipt_destination",
        lambda **kwargs: (tmp_path / "receipt.json") if receipt else None,
    )
    for enabled in (True, False):
        with pytest.raises(_Stop):
            SA.main(["--json"], scan_records=enabled)
    # A release receipt attests a fresh computation; a library call computes.
    assert [records is not None for records in opened] == [not receipt, False]
