from __future__ import annotations

import argparse
import hashlib
import json
import os
import shutil
import sqlite3
import subprocess
import sys
import time
from concurrent.futures import ThreadPoolExecutor
from dataclasses import replace
from pathlib import Path
from threading import Barrier
from types import SimpleNamespace

import pytest

from molt import scientific_stack_versions as scientific_versions
from molt.cli.file_hashing import _sha256_file
from molt.cli.source_build_environment import canonical_source_marker_environment
from molt.cli.source_extension_set_identity import _source_extension_set_identity
from molt.cli.source_package_seal import SourcePackageInput, stage_source_package_seal
from molt.cli.source_package_seal import verify_source_package_seal
from molt.scientific_stack_versions import (
    resolve_scientific_stack,
    scientific_extension_set,
)
from tools.proof_queue_pkg import cli, custody, pact, policy, runner, scheduling, state
from tools.proof_queue_pkg import diagnostics as diagnostics_module
from tools.proof_queue_pkg import evidence as evidence_module

_TEST_GIT_SNAPSHOT = {
    "available": True,
    "head": "test-head",
    "dirty": False,
    "status": [],
    "ignored_status_count": 0,
}
_REAL_GIT_SNAPSHOT_TESTS = {
    "test_proof_queue_git_snapshot_ignores_generated_wasm_checksums",
    "test_proof_queue_git_snapshot_expands_untracked_directories",
}


def test_proof_queue_default_state_is_owned_by_checkout_custody(
    monkeypatch: pytest.MonkeyPatch, tmp_path: Path
) -> None:
    custody_root = tmp_path / "custody"
    monkeypatch.setattr(
        state,
        "checkout_custody",
        lambda _root, _env: SimpleNamespace(
            custody_root=custody_root,
            source_root=tmp_path / "ephemeral-source",
            ephemeral=True,
        ),
    )
    args = argparse.Namespace(db=None, logs_root=None)

    assert state._db_path(args) == (
        custody_root / "logs" / "proof_queue" / "proof_queue.sqlite3"
    )
    assert state._logs_root(args) == (custody_root / "logs" / "proof_queue" / "runs")


def test_proof_queue_durable_state_remains_with_its_source_checkout(
    monkeypatch: pytest.MonkeyPatch, tmp_path: Path
) -> None:
    source_root = tmp_path / "durable-source"
    monkeypatch.setattr(
        state,
        "checkout_custody",
        lambda _root, _env: SimpleNamespace(
            custody_root=tmp_path / "family-custody",
            source_root=source_root,
            ephemeral=False,
        ),
    )
    args = argparse.Namespace(db=None, logs_root=None)

    assert state._db_path(args) == (
        source_root / "logs" / "proof_queue" / "proof_queue.sqlite3"
    )
    assert state._logs_root(args) == source_root / "logs" / "proof_queue" / "runs"


def test_status_keeps_pact_authority_out_of_the_hot_import_path(tmp_path: Path) -> None:
    db = tmp_path / "proof_queue.sqlite3"
    logs = tmp_path / "runs"
    notebooks = tmp_path / "notebooks"
    script = f"""
import sys
from tools.proof_queue_pkg import cli
assert 'tools.proof_queue_pkg.pact' not in sys.modules
rc = cli.main([
    '--db', {str(db)!r},
    '--logs-root', {str(logs)!r},
    '--notebooks-root', {str(notebooks)!r},
    'status',
])
assert rc == 0
assert 'tools.proof_queue_pkg.pact' not in sys.modules
"""
    completed = subprocess.run(
        [sys.executable, "-c", script],
        cwd=state.ROOT,
        check=False,
        capture_output=True,
        text=True,
    )
    assert completed.returncode == 0, completed.stderr


@pytest.fixture(autouse=True)
def _proof_queue_unit_git_snapshot(
    request: pytest.FixtureRequest, monkeypatch: pytest.MonkeyPatch
) -> None:
    # Lane-maturity admission has its own focused test module. Keep the broad
    # queue suite deterministic and scoped to the contract each test names;
    # otherwise a developer's shared .molt registry can reject a synthetic WASM
    # row before mutex, preflight, or detached-runner behavior is exercised.
    monkeypatch.setattr(
        scheduling,
        "_lane_maturity_admission",
        lambda **_kwargs: SimpleNamespace(allow=True, reason="admitted"),
    )
    if request.node.name in _REAL_GIT_SNAPSHOT_TESTS:
        return
    monkeypatch.setattr(state, "_git_snapshot", lambda cwd: _TEST_GIT_SNAPSHOT)


def _rows(db: Path) -> list[sqlite3.Row]:
    conn = sqlite3.connect(db)
    conn.row_factory = sqlite3.Row
    return list(conn.execute("SELECT * FROM proof_runs ORDER BY rowid"))


def test_proof_queue_non_wasm_exec_does_not_load_wasm_toolchain(
    monkeypatch: pytest.MonkeyPatch, tmp_path: Path
) -> None:
    db = tmp_path / "proof_queue.sqlite3"
    logs = tmp_path / "runs"

    def reject_wasm_toolchain_load():
        raise AssertionError("non-WASM queue commands must stay import-light")

    monkeypatch.setattr(policy, "_load_wasm_toolchain", reject_wasm_toolchain_load)

    rc = cli.main(
        [
            "--db",
            str(db),
            "--logs-root",
            str(logs),
            "--repo-root",
            str(state.ROOT),
            "exec",
            "--id",
            "python-import-light",
            "--reason",
            "prove non-wasm queue rows avoid wasm CLI imports",
            "--resource-family",
            "python-tests",
            "--contention-key",
            "python:import-light",
            "--",
            sys.executable,
            "-c",
            "print('ran')",
        ]
    )

    assert rc == 0
    rows = _rows(db)
    assert rows[0]["status"] == "passed"
    assert "ran" in Path(rows[0]["log_path"]).read_text(encoding="utf-8")


def _notes(db: Path) -> list[sqlite3.Row]:
    conn = sqlite3.connect(db)
    conn.row_factory = sqlite3.Row
    return list(conn.execute("SELECT * FROM proof_notes ORDER BY note_id"))


def _edges(db: Path) -> list[sqlite3.Row]:
    conn = sqlite3.connect(db)
    conn.row_factory = sqlite3.Row
    return list(conn.execute("SELECT * FROM proof_run_edges ORDER BY edge_id"))


def _insert_blocked_dependency_fixture(
    db: Path,
    logs: Path,
    *,
    contention_key: str = "python:blocked-slot",
) -> None:
    conn = state._connect(db)
    for run_id, status, key in (
        ("failed-parent", "failed", "python:failed-parent"),
        ("blocked-child", "queued", contention_key),
    ):
        scheduling._insert_run(
            conn,
            run_id=run_id,
            logical_id=run_id,
            reason="prove blocked dependency reconciliation",
            command=[sys.executable, "-c", "print('blocked')"],
            cwd=state.ROOT,
            resource_family="python",
            contention_key=key,
            scopes=["tools/proof_queue.py"],
            git_snapshot={
                "available": True,
                "head": "abc123",
                "dirty": False,
                "status": [],
            },
            log_path=logs / f"{run_id}.log",
            summary_json=logs / f"{run_id}.memory_guard.json",
        )
        values: dict[str, object] = {"status": status}
        if status != "queued":
            values["finished_at"] = state._utc_now()
        state._update_run(conn, run_id, **values)
    state._insert_edge(
        conn,
        parent_run_id="failed-parent",
        child_run_id="blocked-child",
        kind="depends_on",
        note="child waits on failed parent",
    )


def test_proof_queue_session_id_is_contention_key_scoped() -> None:
    assert state._proof_session_id("wasm", "wasm-build") == state._proof_session_id(
        "wasm", "wasm-build"
    )
    assert state._proof_session_id("wasm", "wasm-build") != state._proof_session_id(
        "wasm", "wasm-browser"
    )


def test_proof_queue_pid_alive_detects_current_process() -> None:
    assert custody._pid_alive(os.getpid())
    assert not custody._pid_alive(0)


def test_proof_queue_git_snapshot_ignores_generated_wasm_checksums(
    tmp_path: Path,
) -> None:
    def git(*args: str) -> None:
        subprocess.run(
            ["git", *args],
            cwd=tmp_path,
            check=True,
            stdout=subprocess.PIPE,
            stderr=subprocess.PIPE,
            text=True,
        )

    git("init")
    git("config", "user.email", "test@example.com")
    git("config", "user.name", "Test User")
    (tmp_path / "wasm").mkdir()
    (tmp_path / "src").mkdir()
    (tmp_path / "wasm" / "molt_runtime.wasm.sha256").write_text(
        "old\n", encoding="utf-8"
    )
    (tmp_path / "wasm" / "molt_runtime_reloc.wasm.sha256").write_text(
        "old\n", encoding="utf-8"
    )
    (tmp_path / "wasm" / "molt_runtime_reloc.wasm.wasm-release.sha256").write_text(
        "old\n", encoding="utf-8"
    )
    (tmp_path / "src" / "app.py").write_text("print('ok')\n", encoding="utf-8")
    git("add", ".")
    git("commit", "-m", "init")

    (tmp_path / "wasm" / "molt_runtime.wasm.sha256").write_text(
        "new\n", encoding="utf-8"
    )
    (tmp_path / "wasm" / "molt_runtime_reloc.wasm.wasm-release.sha256").write_text(
        "new\n", encoding="utf-8"
    )
    snapshot = state._git_snapshot(tmp_path)
    assert snapshot["dirty"] is False
    assert snapshot["status"] == []
    assert snapshot["ignored_status_count"] == 2

    (tmp_path / "src" / "app.py").write_text("print('changed')\n", encoding="utf-8")
    snapshot = state._git_snapshot(tmp_path)
    assert snapshot["dirty"] is True
    assert any("src/app.py" in line for line in snapshot["status"])


def test_proof_queue_git_snapshot_expands_untracked_directories(
    tmp_path: Path,
) -> None:
    def git(*args: str) -> None:
        subprocess.run(
            ["git", *args],
            cwd=tmp_path,
            check=True,
            stdout=subprocess.PIPE,
            stderr=subprocess.PIPE,
            text=True,
        )

    git("init")
    git("config", "user.email", "test@example.com")
    git("config", "user.name", "Test User")
    (tmp_path / "tracked.txt").write_text("tracked\n", encoding="utf-8")
    git("add", ".")
    git("commit", "-m", "init")

    split_dir = tmp_path / "src" / "split"
    split_dir.mkdir(parents=True)
    (split_dir / "mod.rs").write_text("mod child;\n", encoding="utf-8")
    (split_dir / "child.rs").write_text("fn child() {}\n", encoding="utf-8")

    snapshot = state._git_snapshot(tmp_path)

    assert snapshot["dirty"] is True
    assert "?? src/split/" not in snapshot["status"]
    assert any("src/split/mod.rs" in line for line in snapshot["status"])
    assert any("src/split/child.rs" in line for line in snapshot["status"])


def test_proof_queue_exec_records_passed_run(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    db = tmp_path / "proof_queue.sqlite3"
    logs = tmp_path / "runs"
    notebooks = tmp_path / "notebooks"
    monkeypatch.setenv("MOLT_MEMORY_GUARD_POLL_SEC", "0.1")

    rc = cli.main(
        [
            "--db",
            str(db),
            "--logs-root",
            str(logs),
            "--notebooks-root",
            str(notebooks),
            "--repo-root",
            str(state.ROOT),
            "exec",
            "--id",
            "queue-smoke",
            "--reason",
            "prove queue smoke",
            "--resource-family",
            "python",
            "--contention-key",
            "python:queue-smoke",
            "--env",
            "PROOF_QUEUE_TEST=queue-ok",
            "--note",
            "changed queue smoke to verify note capture",
            "--timeout",
            "30",
            "--",
            sys.executable,
            "-c",
            "import os; print(os.environ['PROOF_QUEUE_TEST'])",
        ]
    )

    assert rc == 0
    rows = _rows(db)
    assert len(rows) == 1
    assert rows[0]["status"] == "passed"
    assert rows[0]["returncode"] == 0
    log_text = Path(rows[0]["log_path"]).read_text(encoding="utf-8")
    assert "queue-ok" in log_text
    assert "memory_guard_poll_sec=2.0" in log_text
    assert "--poll-interval 2.0" in log_text
    notes = _notes(db)
    assert [note["body"] for note in notes] == [
        "changed queue smoke to verify note capture"
    ]
    notebook = notebooks / f"{rows[0]['run_id']}.py"
    notebook_text = notebook.read_text(encoding="utf-8")
    assert "import marimo" in notebook_text
    assert '"status": "passed"' in notebook_text
    assert "changed queue smoke to verify note capture" in notebook_text
    assert '"note_kind_counts": {' in notebook_text
    assert '"submission": 1' in notebook_text


def test_proof_queue_exec_requires_command_delimiter(tmp_path: Path) -> None:
    db = tmp_path / "proof_queue.sqlite3"
    logs = tmp_path / "runs"

    with pytest.raises(SystemExit, match="requires `--` before the proof command"):
        cli.main(
            [
                "--db",
                str(db),
                "--logs-root",
                str(logs),
                "--repo-root",
                str(state.ROOT),
                "exec",
                "--id",
                "missing-delimiter",
                "--reason",
                "prove delimiter guard",
                sys.executable,
                "-c",
                "print('would run without delimiter')",
            ]
        )

    assert not db.exists()


@pytest.mark.parametrize(
    ("subcommand", "expected"),
    [
        ("exec", "submit and run one inline proof"),
        ("cargo", "submit a queue-owned Cargo proof"),
    ],
)
def test_proof_queue_proof_command_help_does_not_require_delimiter(
    subcommand: str,
    expected: str,
    capsys: pytest.CaptureFixture[str],
) -> None:
    with pytest.raises(SystemExit) as exc:
        cli.main([subcommand, "--help"])

    assert exc.value.code == 0
    captured = capsys.readouterr()
    assert expected in captured.out
    assert "requires `--` before the proof command" not in captured.err


def test_proof_queue_help_detection_ignores_metadata_values_and_command_args() -> None:
    assert cli._proof_command_help_requested(["exec", "--help"])
    assert cli._proof_command_help_requested(["exec", "--id", "help-smoke", "-h"])
    assert not cli._proof_command_help_requested(
        ["exec", "--note", "--help", "--", sys.executable]
    )
    assert not cli._proof_command_help_requested(
        ["exec", "--note=--help", "--", sys.executable]
    )
    assert not cli._proof_command_help_requested(["exec", "--", "--help"])


def test_proof_queue_exec_preserves_command_help_after_delimiter(
    tmp_path: Path, capsys: pytest.CaptureFixture[str]
) -> None:
    db = tmp_path / "proof_queue.sqlite3"
    logs = tmp_path / "runs"

    assert (
        cli.main(
            [
                "--db",
                str(db),
                "--logs-root",
                str(logs),
                "--repo-root",
                str(state.ROOT),
                "exec",
                "--id",
                "command-help",
                "--reason",
                "prove command help stays after delimiter",
                "--resource-family",
                "python",
                "--contention-key",
                "python:command-help",
                "--scope",
                "tools/proof_queue.py",
                "--note",
                "test: delimiter preflight must not consume proof command help",
                "--timeout",
                "30",
                "--",
                sys.executable,
                "-c",
                "import sys; print(sys.argv[1])",
                "--help",
            ]
        )
        == 0
    )

    out = capsys.readouterr().out
    assert "passed" in out
    log_text = next(logs.glob("*.log")).read_text(encoding="utf-8")
    assert "--help" in log_text


def test_proof_queue_exec_rejects_removed_wait_flag(tmp_path: Path) -> None:
    db = tmp_path / "proof_queue.sqlite3"
    logs = tmp_path / "runs"

    with pytest.raises(SystemExit):
        cli.main(
            [
                "--db",
                str(db),
                "--logs-root",
                str(logs),
                "--repo-root",
                str(state.ROOT),
                "exec",
                "--id",
                "removed-wait",
                "--reason",
                "prove removed wait flag",
                "--resource-family",
                "python",
                "--contention-key",
                "python:removed-wait",
                "--wait",
                "--",
                sys.executable,
                "-c",
                "print('must not run')",
            ]
        )

    assert not db.exists()


def test_proof_queue_exec_rejects_pre_delimiter_residue(
    tmp_path: Path,
) -> None:
    db = tmp_path / "proof_queue.sqlite3"
    logs = tmp_path / "runs"

    with pytest.raises(SystemExit, match="stray positional argument"):
        cli.main(
            [
                "--db",
                str(db),
                "--logs-root",
                str(logs),
                "--repo-root",
                str(state.ROOT),
                "exec",
                "--id",
                "bad-shell-quote",
                "--reason",
                '"Prove',
                "molt_dev",
                "fixture",
                "--resource-family",
                "python-tests",
                "--contention-key",
                "molt-dev",
                "--note",
                "this metadata would be silently swallowed before the fix",
                "--",
                sys.executable,
                "-c",
                "print('must not run')",
            ]
        )

    assert not db.exists()


def test_proof_queue_exec_honors_explicit_memory_guard_poll_override(
    tmp_path: Path,
) -> None:
    db = tmp_path / "proof_queue.sqlite3"
    logs = tmp_path / "runs"

    rc = cli.main(
        [
            "--db",
            str(db),
            "--logs-root",
            str(logs),
            "--repo-root",
            str(state.ROOT),
            "exec",
            "--id",
            "queue-poll-override",
            "--reason",
            "prove queue memory guard poll override",
            "--resource-family",
            "python",
            "--contention-key",
            "python:queue-poll-override",
            "--env",
            "MOLT_MEMORY_GUARD_POLL_SEC=0.25",
            "--note",
            "synthetic override: queue wrapper must route the operator poll interval into memory_guard.py",
            "--timeout",
            "30",
            "--",
            sys.executable,
            "-c",
            "import os; print(os.environ['MOLT_MEMORY_GUARD_POLL_SEC'])",
        ]
    )

    rows = _rows(db)
    assert rc == 0
    assert len(rows) == 1
    assert rows[0]["status"] == "passed"
    log_text = Path(rows[0]["log_path"]).read_text(encoding="utf-8")
    assert "0.25" in log_text
    assert "memory_guard_poll_sec=0.25" in log_text
    assert "--poll-interval 0.25" in log_text


def test_proof_queue_rejects_invalid_memory_guard_poll_override() -> None:
    with pytest.raises(ValueError, match="MOLT_MEMORY_GUARD_POLL_SEC"):
        custody._proof_queue_memory_guard_poll_sec(
            {"MOLT_MEMORY_GUARD_POLL_SEC": "not-a-number"}
        )
    with pytest.raises(ValueError, match="MOLT_MEMORY_GUARD_POLL_SEC"):
        custody._proof_queue_memory_guard_poll_sec({"MOLT_MEMORY_GUARD_POLL_SEC": "0"})


def test_proof_queue_exec_rejects_invalid_memory_guard_poll_before_detach(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    db = tmp_path / "proof_queue.sqlite3"
    logs = tmp_path / "runs"
    launched: list[str] = []

    def fake_launch(args: object, *, run_id: str, timeout: float) -> tuple[int, Path]:
        del args, timeout
        launched.append(run_id)
        return 4242, tmp_path / "runner.log"

    monkeypatch.setattr(custody, "_launch_detached_runner", fake_launch)

    rc = cli.main(
        [
            "--db",
            str(db),
            "--logs-root",
            str(logs),
            "--repo-root",
            str(state.ROOT),
            "exec",
            "--id",
            "bad-poll",
            "--reason",
            "reject invalid poll interval",
            "--resource-family",
            "python",
            "--contention-key",
            "python:bad-poll",
            "--env",
            "MOLT_MEMORY_GUARD_POLL_SEC=not-a-number",
            "--note",
            "synthetic violation: invalid poll interval must fail before detached runner launch",
            "--timeout",
            "30",
            "--detach",
            "--",
            sys.executable,
            "-c",
            "print('must-not-run')",
        ]
    )

    rows = _rows(db)
    assert rc == 2
    assert len(rows) == 1
    assert rows[0]["status"] == "failed"
    assert rows[0]["returncode"] == 2
    assert launched == []
    log_text = Path(rows[0]["log_path"]).read_text(encoding="utf-8")
    assert "proof queue refuses invalid environment override" in log_text
    assert "MOLT_MEMORY_GUARD_POLL_SEC" in log_text


def test_proof_queue_evidence_accepts_positional_run_id(
    tmp_path: Path,
    capsys: pytest.CaptureFixture[str],
) -> None:
    db = tmp_path / "proof_queue.sqlite3"
    logs = tmp_path / "runs"
    notebooks = tmp_path / "notebooks"
    base_args = [
        "--db",
        str(db),
        "--logs-root",
        str(logs),
        "--notebooks-root",
        str(notebooks),
        "--repo-root",
        str(state.ROOT),
    ]
    assert (
        cli.main(
            [
                *base_args,
                "exec",
                "--id",
                "evidence-smoke",
                "--reason",
                "prove evidence run id selectors",
                "--resource-family",
                "python",
                "--contention-key",
                "python:evidence-smoke",
                "--timeout",
                "30",
                "--",
                sys.executable,
                "-c",
                "print('ok')",
            ]
        )
        == 0
    )
    run_id = _rows(db)[0]["run_id"]

    capsys.readouterr()
    assert cli.main([*base_args, "evidence", run_id]) == 0
    positional_payload = json.loads(capsys.readouterr().out)
    assert [item["run_id"] for item in positional_payload] == [run_id]

    assert cli.main([*base_args, "evidence", "--run-id", run_id]) == 0
    flag_payload = json.loads(capsys.readouterr().out)
    assert [item["run_id"] for item in flag_payload] == [run_id]

    with pytest.raises(SystemExit, match="unknown proof run id"):
        cli.main([*base_args, "evidence", "not-a-run-id"])

    with pytest.raises(SystemExit, match="positional and --run-id disagree"):
        cli.main([*base_args, "evidence", run_id, "--run-id", "not-a-run-id"])


def test_proof_queue_projection_failure_is_nonfatal_observability(
    monkeypatch: pytest.MonkeyPatch,
    tmp_path: Path,
    capsys: pytest.CaptureFixture[str],
) -> None:
    db = tmp_path / "proof_queue.sqlite3"
    logs = tmp_path / "runs"
    marker = tmp_path / "proof-ran.txt"

    def fail_notebook(*_args: object, **_kwargs: object) -> Path:
        raise RuntimeError("notebook projection exploded")

    monkeypatch.setattr(evidence_module, "_write_marimo_notebook", fail_notebook)

    rc = cli.main(
        [
            "--db",
            str(db),
            "--logs-root",
            str(logs),
            "--repo-root",
            str(state.ROOT),
            "exec",
            "--id",
            "projection-warning",
            "--reason",
            "prove notebook projection failure does not block proof execution",
            "--resource-family",
            "python",
            "--contention-key",
            "python:projection-warning",
            "--note",
            "trigger projection before command execution but still run",
            "--",
            sys.executable,
            "-c",
            "from pathlib import Path; import sys; Path(sys.argv[1]).write_text('ran')",
            str(marker),
        ]
    )

    assert rc == 0
    assert marker.read_text(encoding="utf-8") == "ran"
    rows = _rows(db)
    assert len(rows) == 1
    assert rows[0]["status"] == "passed"
    assert rows[0]["returncode"] == 0
    log_text = Path(rows[0]["log_path"]).read_text(encoding="utf-8")
    assert (
        "proof queue nonfatal infrastructure failure during submission projection"
        in log_text
    )
    assert "RuntimeError: notebook projection exploded" in log_text
    assert "--- proof_queue command execution ---" in log_text

    capsys.readouterr()
    assert (
        cli.main(
            [
                "--db",
                str(db),
                "--logs-root",
                str(logs),
                "--repo-root",
                str(state.ROOT),
                "evidence",
                "--run-id",
                rows[0]["run_id"],
            ]
        )
        == 0
    )
    evidence = json.loads(capsys.readouterr().out)
    signals = {item["signal_id"] for item in evidence[0]["diagnostics"]}
    assert "queue-infra-warning" in signals


def test_proof_queue_submission_metadata_failure_is_terminal(
    monkeypatch: pytest.MonkeyPatch,
    tmp_path: Path,
    capsys: pytest.CaptureFixture[str],
) -> None:
    db = tmp_path / "proof_queue.sqlite3"
    logs = tmp_path / "runs"
    marker = tmp_path / "should-not-run.txt"
    followup_marker = tmp_path / "followup-ran.txt"

    def fail_insert_note(*_args: object, **_kwargs: object) -> int:
        raise RuntimeError("note insert exploded")

    monkeypatch.setattr(state, "_insert_note", fail_insert_note)

    rc = cli.main(
        [
            "--db",
            str(db),
            "--logs-root",
            str(logs),
            "--repo-root",
            str(state.ROOT),
            "exec",
            "--id",
            "metadata-crash",
            "--reason",
            "prove submission metadata failure is terminal",
            "--resource-family",
            "python",
            "--contention-key",
            "python:metadata-crash",
            "--note",
            "trigger metadata failure before command execution",
            "--",
            sys.executable,
            "-c",
            "from pathlib import Path; import sys; Path(sys.argv[1]).write_text('ran')",
            str(marker),
        ]
    )

    assert rc == 2
    assert not marker.exists()
    rows = _rows(db)
    assert len(rows) == 1
    assert rows[0]["status"] == "failed"
    assert rows[0]["returncode"] == 2
    log_text = Path(rows[0]["log_path"]).read_text(encoding="utf-8")
    assert (
        "proof queue fatal infrastructure failure during submission metadata"
        in log_text
    )
    assert "RuntimeError: note insert exploded" in log_text

    capsys.readouterr()
    assert (
        cli.main(
            [
                "--db",
                str(db),
                "--logs-root",
                str(logs),
                "--repo-root",
                str(state.ROOT),
                "evidence",
                "--run-id",
                rows[0]["run_id"],
            ]
        )
        == 0
    )
    evidence = json.loads(capsys.readouterr().out)
    signals = [item["signal_id"] for item in evidence[0]["diagnostics"]]
    assert signals[0] == "queue-preexecution-failure"
    assert "queue-infra-warning" not in signals

    rc = cli.main(
        [
            "--db",
            str(db),
            "--logs-root",
            str(logs),
            "--repo-root",
            str(state.ROOT),
            "exec",
            "--id",
            "metadata-followup",
            "--reason",
            "prove contention key is released",
            "--resource-family",
            "python",
            "--contention-key",
            "python:metadata-crash",
            "--",
            sys.executable,
            "-c",
            "from pathlib import Path; import sys; Path(sys.argv[1]).write_text('ran')",
            str(followup_marker),
        ]
    )

    assert rc == 0
    assert followup_marker.read_text(encoding="utf-8") == "ran"


def test_proof_queue_refuses_duplicate_active_contention_key(tmp_path: Path) -> None:
    db = tmp_path / "proof_queue.sqlite3"
    conn = state._connect(db)
    scheduling._insert_run(
        conn,
        run_id="active-run",
        logical_id="active",
        reason="already running",
        command=[sys.executable, "-c", "print('active')"],
        cwd=state.ROOT,
        resource_family="python",
        contention_key="python:shared",
        scopes=[],
        log_path=tmp_path / "active.log",
        summary_json=tmp_path / "active.memory_guard.json",
    )
    state._update_run(
        conn,
        "active-run",
        status="running",
        started_at=state._utc_now(),
    )

    rc = cli.main(
        [
            "--db",
            str(db),
            "--logs-root",
            str(tmp_path / "runs"),
            "--repo-root",
            str(state.ROOT),
            "exec",
            "--id",
            "blocked",
            "--reason",
            "should not run",
            "--resource-family",
            "python",
            "--contention-key",
            "python:shared",
            "--",
            sys.executable,
            "-c",
            "raise SystemExit(99)",
        ]
    )

    assert rc == 2
    assert len(_rows(db)) == 1


def test_proof_queue_refuses_concurrent_compiler_build_resource_mutex(
    tmp_path: Path, capsys: pytest.CaptureFixture[str]
) -> None:
    db = tmp_path / "proof_queue.sqlite3"
    conn = state._connect(db)
    scheduling._insert_run(
        conn,
        run_id="active-pact",
        logical_id="active-pact",
        reason="browser witness already building runtime",
        command=[sys.executable, "tools/pact_witness_acceptance.py"],
        cwd=state.ROOT,
        resource_family="wasm-browser",
        contention_key="wasm:pact-witness",
        scopes=[],
        log_path=tmp_path / "active-pact.log",
        summary_json=tmp_path / "active-pact.memory_guard.json",
    )
    state._update_run(
        conn,
        "active-pact",
        status="running",
        started_at=state._utc_now(),
    )

    rc = cli.main(
        [
            "--db",
            str(db),
            "--logs-root",
            str(tmp_path / "runs"),
            "--repo-root",
            str(state.ROOT),
            "exec",
            "--id",
            "r4a-probe",
            "--reason",
            "should not overlap runtime wasm build",
            "--resource-family",
            "wasm",
            "--contention-key",
            "wasm:r4a-lirfast",
            "--",
            sys.executable,
            "-c",
            "raise SystemExit(99)",
        ]
    )

    stderr = capsys.readouterr().err
    assert rc == 2
    assert len(_rows(db)) == 1
    assert (
        "resource mutex 'compiler-build-resource' already has active run(s)" in stderr
    )
    assert "active-pact" in stderr


def test_proof_queue_status_shows_active_log_phase(
    tmp_path: Path, capsys: pytest.CaptureFixture[str]
) -> None:
    db = tmp_path / "proof_queue.sqlite3"
    log_path = tmp_path / "active.log"
    log_path.write_text(
        "proof_queue run_id=active-run\n"
        "\n"
        "Runtime wasm build: still running elapsed=120s timeout=unbounded pid=123\n",
        encoding="utf-8",
    )
    conn = state._connect(db)
    scheduling._insert_run(
        conn,
        run_id="active-run",
        logical_id="active",
        reason="show active phase",
        command=[sys.executable, "-c", "print('active')"],
        cwd=state.ROOT,
        resource_family="wasm",
        contention_key="wasm-build",
        scopes=[],
        log_path=log_path,
        summary_json=tmp_path / "active.memory_guard.json",
    )
    state._update_run(conn, "active-run", status="running", started_at=state._utc_now())

    assert (
        cli.main(
            [
                "--db",
                str(db),
                "--logs-root",
                str(tmp_path / "runs"),
                "--repo-root",
                str(state.ROOT),
                "status",
                "--recent",
                "0",
            ]
        )
        == 0
    )

    out = capsys.readouterr().out
    assert f"log={log_path}" in out
    assert "last_log_age=" in out
    assert "Runtime wasm build: still running elapsed=120s" in out


def test_proof_queue_status_shows_active_pytest_current_test(
    tmp_path: Path, capsys: pytest.CaptureFixture[str]
) -> None:
    db = tmp_path / "proof_queue.sqlite3"
    log_path = tmp_path / "active.log"
    summary_path = tmp_path / "active.memory_guard.json"
    current_path = tmp_path / "pytest-current.json"
    nodeid = "tests/test_molt_dev.py::test_cleanup_force_requires_matching_sha"
    log_path.write_text("proof_queue run_id=active-run\n", encoding="utf-8")
    summary_path.write_text(
        json.dumps(
            {
                "status": "child_running",
                "pytest": {
                    "current_test_file": {
                        "path": str(current_path),
                        "payload": {"nodeid": nodeid, "phase": "setup"},
                    }
                },
            }
        ),
        encoding="utf-8",
    )
    conn = state._connect(db)
    scheduling._insert_run(
        conn,
        run_id="active-run",
        logical_id="active",
        reason="show active pytest phase",
        command=[sys.executable, "-m", "pytest", "tests/test_molt_dev.py"],
        cwd=state.ROOT,
        resource_family="python-tests",
        contention_key="molt-dev",
        scopes=[],
        log_path=log_path,
        summary_json=summary_path,
    )
    state._update_run(conn, "active-run", status="running", started_at=state._utc_now())

    assert (
        cli.main(
            [
                "--db",
                str(db),
                "--logs-root",
                str(tmp_path / "runs"),
                "--repo-root",
                str(state.ROOT),
                "status",
                "--recent",
                "0",
            ]
        )
        == 0
    )

    out = capsys.readouterr().out
    assert f"pytest_current={nodeid} phase=setup" in out


def test_proof_queue_status_hides_pytest_current_for_non_pytest_rows(
    tmp_path: Path, capsys: pytest.CaptureFixture[str]
) -> None:
    db = tmp_path / "proof_queue.sqlite3"
    log_path = tmp_path / "active.log"
    summary_path = tmp_path / "active.memory_guard.json"
    current_path = tmp_path / "pytest-current.json"
    log_path.write_text("proof_queue run_id=active-run\n", encoding="utf-8")
    summary_path.write_text(
        json.dumps(
            {
                "status": "child_running",
                "pytest": {
                    "current_test_file": {
                        "missing": True,
                        "path": str(current_path),
                    }
                },
            }
        ),
        encoding="utf-8",
    )
    conn = state._connect(db)
    scheduling._insert_run(
        conn,
        run_id="active-run",
        logical_id="active",
        reason="show non-pytest rows do not inherit pytest custody noise",
        command=[sys.executable, "tests/molt_diff.py", "--jobs", "1"],
        cwd=state.ROOT,
        resource_family="python",
        contention_key="python:r6",
        scopes=[],
        log_path=log_path,
        summary_json=summary_path,
    )
    state._update_run(conn, "active-run", status="running", started_at=state._utc_now())

    assert (
        cli.main(
            [
                "--db",
                str(db),
                "--logs-root",
                str(tmp_path / "runs"),
                "--repo-root",
                str(state.ROOT),
                "status",
                "--recent",
                "0",
            ]
        )
        == 0
    )

    out = capsys.readouterr().out
    assert f"log={log_path}" in out
    assert "pytest_current=" not in out
    assert str(current_path) not in out


def test_proof_queue_diagnoses_running_pytest_missing_current_test_file(
    tmp_path: Path, capsys: pytest.CaptureFixture[str]
) -> None:
    db = tmp_path / "proof_queue.sqlite3"
    log_path = tmp_path / "active.log"
    summary_path = tmp_path / "active.memory_guard.json"
    current_path = tmp_path / "pytest-current.json"
    log_path.write_text("proof_queue run_id=active-run\n", encoding="utf-8")
    stale = (
        time.time()
        - diagnostics_module.RUNNING_PYTEST_CURRENT_TEST_MISSING_STALE_SECONDS
        - 5.0
    )
    os.utime(log_path, (stale, stale))
    summary_path.write_text(
        json.dumps(
            {
                "status": "child_running",
                "child_process": {
                    "pid": 3210,
                    "command": [
                        sys.executable,
                        str(state.ROOT / "tools" / "memory_guard.py"),
                    ],
                },
                "repro": {"host": {"platform": "win32"}},
                "pytest": {
                    "current_test_file": {
                        "missing": True,
                        "path": str(current_path),
                    }
                },
            }
        ),
        encoding="utf-8",
    )
    conn = state._connect(db)
    scheduling._insert_run(
        conn,
        run_id="active-run",
        logical_id="active",
        reason="diagnose quiet pytest startup",
        command=[sys.executable, "-m", "pytest", "tests/test_molt_dev.py"],
        cwd=state.ROOT,
        resource_family="python-tests",
        contention_key="molt-dev",
        scopes=["tools/proof_queue.py"],
        log_path=log_path,
        summary_json=summary_path,
    )
    state._update_run(conn, "active-run", status="running", started_at=state._utc_now())

    assert (
        cli.main(
            [
                "--db",
                str(db),
                "--logs-root",
                str(tmp_path / "runs"),
                "--repo-root",
                str(state.ROOT),
                "diagnose",
                "active-run",
            ]
        )
        == 0
    )

    out = capsys.readouterr().out
    assert "running-pytest-current-test-missing" in out
    assert str(current_path) in out
    assert "windows_memory_guard_child_runner pid=3210" in out
    assert "pre-test or collection/startup opacity" in out
    assert "uv/cache contention" in out


def test_proof_queue_audit_warns_on_running_pytest_missing_current_test_file(
    tmp_path: Path, capsys: pytest.CaptureFixture[str]
) -> None:
    db = tmp_path / "proof_queue.sqlite3"
    log_path = tmp_path / "active.log"
    summary_path = tmp_path / "active.memory_guard.json"
    current_path = tmp_path / "pytest-current.json"
    log_path.write_text("proof_queue run_id=active-run\n", encoding="utf-8")
    stale = (
        time.time()
        - diagnostics_module.RUNNING_PYTEST_CURRENT_TEST_MISSING_STALE_SECONDS
        - 5.0
    )
    os.utime(log_path, (stale, stale))
    summary_path.write_text(
        json.dumps(
            {
                "status": "child_running",
                "child_process": {
                    "pid": 3210,
                    "command": [
                        sys.executable,
                        str(state.ROOT / "tools" / "memory_guard.py"),
                    ],
                },
                "repro": {"host": {"platform": "win32"}},
                "pytest": {
                    "current_test_file": {
                        "missing": True,
                        "path": str(current_path),
                    }
                },
            }
        ),
        encoding="utf-8",
    )
    conn = state._connect(db)
    scheduling._insert_run(
        conn,
        run_id="active-run",
        logical_id="active",
        reason="audit quiet pytest startup",
        command=[sys.executable, "-m", "pytest", "tests/test_molt_dev.py"],
        cwd=state.ROOT,
        resource_family="python-tests",
        contention_key="molt-dev",
        scopes=["tools/proof_queue.py"],
        log_path=log_path,
        summary_json=summary_path,
    )
    state._insert_note(
        conn,
        run_id="active-run",
        body="test: audit must surface current-test custody opacity",
        kind="submission",
        author="codex",
    )
    state._update_run(
        conn,
        "active-run",
        status="running",
        started_at=state._utc_now(),
        guard_pid=os.getpid(),
    )

    assert (
        cli.main(
            [
                "--db",
                str(db),
                "--logs-root",
                str(tmp_path / "runs"),
                "--repo-root",
                str(state.ROOT),
                "audit",
                "--no-notebook-check",
            ]
        )
        == 0
    )
    output = capsys.readouterr().out
    assert "running-pytest-current-test-missing=1" in output
    assert "audit-running-pytest-current-test-missing run=active-run" in output
    assert "pre-test or collection/startup opacity" in output


def test_proof_queue_diagnoses_running_pytest_progress_without_current_marker(
    tmp_path: Path, capsys: pytest.CaptureFixture[str]
) -> None:
    db = tmp_path / "proof_queue.sqlite3"
    log_path = tmp_path / "active.log"
    summary_path = tmp_path / "active.memory_guard.json"
    current_path = tmp_path / "pytest-current.json"
    log_path.write_text("proof_queue run_id=active-run\n.\n", encoding="utf-8")
    stale = (
        time.time()
        - diagnostics_module.RUNNING_PYTEST_CURRENT_TEST_MISSING_STALE_SECONDS
        - 5.0
    )
    os.utime(log_path, (stale, stale))
    summary_path.write_text(
        json.dumps(
            {
                "status": "child_running",
                "pytest": {
                    "current_test_file": {
                        "missing": True,
                        "path": str(current_path),
                    }
                },
            }
        ),
        encoding="utf-8",
    )
    conn = state._connect(db)
    scheduling._insert_run(
        conn,
        run_id="active-run",
        logical_id="active",
        reason="diagnose pytest progress without current marker",
        command=[sys.executable, "-m", "pytest", "tests/tools/test_proof_queue.py"],
        cwd=state.ROOT,
        resource_family="python-tests",
        contention_key="proof-queue",
        scopes=["tools/proof_queue.py"],
        log_path=log_path,
        summary_json=summary_path,
    )
    state._update_run(conn, "active-run", status="running", started_at=state._utc_now())

    assert (
        cli.main(
            [
                "--db",
                str(db),
                "--logs-root",
                str(tmp_path / "runs"),
                "--repo-root",
                str(state.ROOT),
                "diagnose",
                "active-run",
            ]
        )
        == 0
    )

    out = capsys.readouterr().out
    assert "running-pytest-current-test-missing" in out
    assert "emitted progress output" in out
    assert "last_pytest_progress=." in out
    assert "current-test custody opacity after pytest started" in out
    assert "running-pytest-failures-observed" not in out
    assert "pre-test or collection/startup opacity" not in out


def test_proof_queue_prioritizes_running_pytest_failure_progress(
    tmp_path: Path, capsys: pytest.CaptureFixture[str]
) -> None:
    db = tmp_path / "proof_queue.sqlite3"
    log_path = tmp_path / "active.log"
    summary_path = tmp_path / "active.memory_guard.json"
    current_path = tmp_path / "pytest-current.json"
    progress = "..FFF.....FF......FF..FF................"
    log_path.write_text(
        f"proof_queue run_id=active-run\n{progress}\n", encoding="utf-8"
    )
    stale = (
        time.time()
        - diagnostics_module.RUNNING_PYTEST_CURRENT_TEST_MISSING_STALE_SECONDS
        - 5.0
    )
    os.utime(log_path, (stale, stale))
    summary_path.write_text(
        json.dumps(
            {
                "status": "child_running",
                "pytest": {
                    "current_test_file": {
                        "missing": True,
                        "path": str(current_path),
                    }
                },
            }
        ),
        encoding="utf-8",
    )
    conn = state._connect(db)
    scheduling._insert_run(
        conn,
        run_id="active-run",
        logical_id="active",
        reason="diagnose pytest failure progress before final report",
        command=[sys.executable, "-m", "pytest", "tests/tools/test_proof_queue.py"],
        cwd=state.ROOT,
        resource_family="python-tests",
        contention_key="proof-queue",
        scopes=["tools/proof_queue.py"],
        log_path=log_path,
        summary_json=summary_path,
    )
    state._update_run(conn, "active-run", status="running", started_at=state._utc_now())

    assert (
        cli.main(
            [
                "--db",
                str(db),
                "--logs-root",
                str(tmp_path / "runs"),
                "--repo-root",
                str(state.ROOT),
                "status",
                "--recent",
                "0",
            ]
        )
        == 0
    )

    status_out = capsys.readouterr().out
    assert "diagnosis=running-pytest-failures-observed [warning]" in status_out
    assert "diagnosis=running-pytest-current-test-missing [infra]" not in status_out

    assert (
        cli.main(
            [
                "--db",
                str(db),
                "--logs-root",
                str(tmp_path / "runs"),
                "--repo-root",
                str(state.ROOT),
                "diagnose",
                "active-run",
            ]
        )
        == 0
    )

    diagnose_out = capsys.readouterr().out
    assert "running-pytest-failures-observed" in diagnose_out
    assert (
        "last_pytest_progress=..FFF.....FF......FF..FF................" in diagnose_out
    )
    assert "failures=9" in diagnose_out
    assert "errors=0" in diagnose_out
    assert "Keep the row running for the full pytest failure report" in diagnose_out
    assert "running-pytest-current-test-missing" in diagnose_out


def test_proof_queue_diagnoses_running_nested_guard_without_work_child(
    monkeypatch: pytest.MonkeyPatch,
    tmp_path: Path,
    capsys: pytest.CaptureFixture[str],
) -> None:
    from tools import memory_guard

    db = tmp_path / "proof_queue.sqlite3"
    log_path = tmp_path / "active.log"
    summary_path = tmp_path / "active.memory_guard.json"
    child_pid = 432_100
    log_path.write_text(
        "proof_queue run_id=active-run\n"
        "source-recompiled external native packages use package/native artifact custody\n",
        encoding="utf-8",
    )
    stale = (
        time.time() - diagnostics_module.RUNNING_CHILD_MISSING_STALE_LOG_SECONDS - 5.0
    )
    os.utime(log_path, (stale, stale))
    summary_path.write_text(
        json.dumps(
            {
                "status": "child_running",
                "child_process": {
                    "pid": child_pid,
                    "command": [
                        sys.executable,
                        str(state.ROOT / "tools" / "memory_guard.py"),
                    ],
                },
            }
        ),
        encoding="utf-8",
    )
    monkeypatch.setattr(custody, "_pid_alive", lambda pid: pid in {child_pid, 99_001})
    monkeypatch.setattr(memory_guard, "sample_processes", lambda: {})
    monkeypatch.setattr(memory_guard, "descendant_pids", lambda samples, pid: set())
    conn = state._connect(db)
    scheduling._insert_run(
        conn,
        run_id="active-run",
        logical_id="active",
        reason="prove stale nested guard diagnosis",
        command=[sys.executable, "-c", "print('active')"],
        cwd=state.ROOT,
        resource_family="wasm",
        contention_key="wasm-build",
        scopes=["tools/proof_queue.py"],
        log_path=log_path,
        summary_json=summary_path,
    )
    state._update_run(
        conn,
        "active-run",
        status="running",
        guard_pid=99_001,
        started_at=state._utc_now(),
    )

    assert (
        cli.main(
            [
                "--db",
                str(db),
                "--logs-root",
                str(tmp_path / "runs"),
                "--repo-root",
                str(state.ROOT),
                "diagnose",
                "active-run",
            ]
        )
        == 0
    )

    out = capsys.readouterr().out
    assert "running-proof-child-missing" in out
    assert "descendants=0" in out
    assert str(summary_path) in out


def test_proof_queue_diagnoses_stale_running_log_with_live_work_child(
    monkeypatch: pytest.MonkeyPatch,
    tmp_path: Path,
    capsys: pytest.CaptureFixture[str],
) -> None:
    from tools import memory_guard

    db = tmp_path / "proof_queue.sqlite3"
    log_path = tmp_path / "active.log"
    summary_path = tmp_path / "active.memory_guard.json"
    child_pid = 432_100
    conhost_pid = 432_099
    work_pid = 432_101
    compile_pid = 432_102
    linker_pid = 432_103
    log_path.write_text(
        "proof_queue run_id=active-run\n"
        "memory_guard_command='python tools/memory_guard.py -- cargo test'\n",
        encoding="utf-8",
    )
    stale = (
        time.time() - diagnostics_module.RUNNING_CHILD_MISSING_STALE_LOG_SECONDS - 5.0
    )
    os.utime(log_path, (stale, stale))
    summary_path.write_text(
        json.dumps(
            {
                "status": "child_running",
                "child_process": {
                    "pid": child_pid,
                    "command": [
                        sys.executable,
                        str(state.ROOT / "tools" / "memory_guard.py"),
                    ],
                },
            }
        ),
        encoding="utf-8",
    )
    monkeypatch.setattr(custody, "_pid_alive", lambda pid: pid == child_pid)
    monkeypatch.setattr(
        memory_guard,
        "sample_processes",
        lambda: {
            conhost_pid: memory_guard.ProcessSample(
                pid=conhost_pid,
                ppid=child_pid,
                rss_kb=256,
                command="C:\\Windows\\System32\\conhost.exe",
            ),
            work_pid: memory_guard.ProcessSample(
                pid=work_pid,
                ppid=child_pid,
                rss_kb=2048,
                command="uv run --active --project . --python 3.12 pytest tests/example.py",
            ),
            compile_pid: memory_guard.ProcessSample(
                pid=compile_pid,
                ppid=work_pid,
                rss_kb=4096,
                command="rustc --crate-name molt_runtime very long command that will be shortened",
            ),
            linker_pid: memory_guard.ProcessSample(
                pid=linker_pid,
                ppid=work_pid,
                rss_kb=1024,
                command="link.exe /OUT:molt.exe",
            ),
        },
    )
    monkeypatch.setattr(
        memory_guard,
        "descendant_pids",
        lambda samples, pid: {conhost_pid, work_pid, compile_pid, linker_pid, 432_104},
    )
    conn = state._connect(db)
    scheduling._insert_run(
        conn,
        run_id="active-run",
        logical_id="active",
        reason="prove stale live-child diagnosis",
        command=[sys.executable, "-c", "print('active')"],
        cwd=state.ROOT,
        resource_family="rust",
        contention_key="cargo-molt-runtime",
        scopes=["tools/proof_queue.py"],
        log_path=log_path,
        summary_json=summary_path,
    )
    state._update_run(
        conn,
        "active-run",
        status="running",
        guard_pid=99_001,
        started_at=state._utc_now(),
    )

    assert (
        cli.main(
            [
                "--db",
                str(db),
                "--logs-root",
                str(tmp_path / "runs"),
                "--repo-root",
                str(state.ROOT),
                "diagnose",
                "active-run",
            ]
        )
        == 0
    )

    out = capsys.readouterr().out
    assert "running-proof-log-stale-live-child" in out
    assert "descendants=5" in out
    assert "descendant_samples=" in out
    assert "conhost.exe" not in out
    assert f"{work_pid}:uv run --active" in out
    assert f"{compile_pid}:rustc --crate-name molt_runtime" in out
    assert "+2 more" in out
    assert "Do not prune or interrupt" in out

    assert (
        cli.main(
            [
                "--db",
                str(db),
                "--logs-root",
                str(tmp_path / "runs"),
                "--repo-root",
                str(state.ROOT),
                "status",
                "--recent",
                "0",
            ]
        )
        == 0
    )

    status_out = capsys.readouterr().out
    assert "guard_descendants=5" in status_out
    assert "descendant_samples=" in status_out
    assert "conhost.exe" not in status_out
    assert f"{work_pid}:uv run --active" in status_out
    assert f"{compile_pid}:rustc --crate-name molt_runtime" in status_out
    assert "+2 more" in status_out


def test_proof_queue_diagnoses_stale_running_launch_summary(
    tmp_path: Path,
    capsys: pytest.CaptureFixture[str],
) -> None:
    db = tmp_path / "proof_queue.sqlite3"
    log_path = tmp_path / "active.log"
    summary_path = tmp_path / "active.memory_guard.json"
    log_path.write_text(
        "proof_queue run_id=active-run\n"
        "Backend compilation: still running elapsed=60s\n"
        " done\n",
        encoding="utf-8",
    )
    stale = (
        time.time() - diagnostics_module.RUNNING_CHILD_MISSING_STALE_LOG_SECONDS - 5.0
    )
    os.utime(log_path, (stale, stale))
    summary_path.write_text(
        json.dumps(
            {
                "status": "running",
                "child_process": None,
                "returncode": None,
                "recorded_at": "2026-07-02T16:03:19Z",
            }
        ),
        encoding="utf-8",
    )
    conn = state._connect(db)
    scheduling._insert_run(
        conn,
        run_id="active-run",
        logical_id="active",
        reason="prove stale launch summary diagnosis",
        command=[sys.executable, "-c", "print('active')"],
        cwd=state.ROOT,
        resource_family="wasm",
        contention_key="wasm-build",
        scopes=["tools/proof_queue.py"],
        log_path=log_path,
        summary_json=summary_path,
    )
    state._update_run(
        conn,
        "active-run",
        status="running",
        guard_pid=99_001,
        started_at=state._utc_now(),
    )

    assert (
        cli.main(
            [
                "--db",
                str(db),
                "--logs-root",
                str(tmp_path / "runs"),
                "--repo-root",
                str(state.ROOT),
                "diagnose",
                "active-run",
            ]
        )
        == 0
    )

    out = capsys.readouterr().out
    assert "running-proof-launch-summary-stale" in out
    assert "child_process=null" in out
    assert str(summary_path) in out


def test_proof_queue_diagnoses_terminal_row_with_unfinished_guard_summary(
    tmp_path: Path,
    capsys: pytest.CaptureFixture[str],
) -> None:
    db = tmp_path / "proof_queue.sqlite3"
    log_path = tmp_path / "failed.log"
    summary_path = tmp_path / "failed.memory_guard.json"
    log_path.write_text(
        "proof_queue run_id=failed-run\n"
        "error[E0308]: synthetic compiler failure masked by custody loss\n"
        "proof_queue finished status=failed exit_code=15 elapsed=18.812s\n",
        encoding="utf-8",
    )
    summary_path.write_text(
        json.dumps(
            {
                "status": "child_running",
                "returncode": None,
                "recorded_at": "2026-07-02T20:47:16Z",
                "repro": {"limits": {"timeout_s": 3600.0}},
                "child_process": {
                    "pid": 22_068,
                    "command": [
                        sys.executable,
                        str(state.ROOT / "tools" / "memory_guard.py"),
                    ],
                },
            }
        ),
        encoding="utf-8",
    )
    conn = state._connect(db)
    scheduling._insert_run(
        conn,
        run_id="failed-run",
        logical_id="failed",
        reason="prove terminal unfinished guard summaries are classified",
        command=[sys.executable, "-c", "print('failed')"],
        cwd=state.ROOT,
        resource_family="python",
        contention_key="python-proof",
        scopes=["tools/proof_queue.py"],
        log_path=log_path,
        summary_json=summary_path,
    )
    state._insert_note(
        conn,
        run_id="failed-run",
        body="test: terminal memory_guard summary must not collapse to unclassified",
        kind="submission",
        author="codex",
    )
    state._update_run(
        conn,
        "failed-run",
        status="failed",
        returncode=15,
        elapsed_s=18.812,
    )

    assert (
        cli.main(
            [
                "--db",
                str(db),
                "--logs-root",
                str(tmp_path / "runs"),
                "--repo-root",
                str(state.ROOT),
                "diagnose",
                "failed-run",
            ]
        )
        == 0
    )

    out = capsys.readouterr().out
    assert "memory-guard-summary-incomplete" in out
    assert "summary_status=child_running" in out
    assert "row_elapsed=18.8s" in out
    assert "configured_timeout=1.0h" in out
    assert "child_process=memory_guard_child_process pid=22068" in out
    assert "last_log=proof_queue finished status=failed exit_code=15" in out
    assert "rust-compiler-error" in out
    assert out.index("memory-guard-summary-incomplete") < out.index(
        "rust-compiler-error"
    )
    assert "queue-custody incomplete" in out
    assert "unclassified-failed-proof" not in out

    assert (
        cli.main(
            [
                "--db",
                str(db),
                "--logs-root",
                str(tmp_path / "runs"),
                "--repo-root",
                str(state.ROOT),
                "audit",
                "--no-notebook-check",
            ]
        )
        == 1
    )
    out = capsys.readouterr().out
    assert "audit-memory-guard-summary-incomplete run=failed-run" in out
    assert "frontier:" not in out
    assert "audit-unclassified-failure" not in out


def test_proof_queue_diagnoses_worker_exit_without_final_summary(
    tmp_path: Path,
    capsys: pytest.CaptureFixture[str],
) -> None:
    db = tmp_path / "proof_queue.sqlite3"
    log_path = tmp_path / "failed.log"
    summary_path = tmp_path / "failed.memory_guard.json"
    log_path.write_text(
        "proof_queue run_id=failed-run\n"
        "proof_queue finished status=failed exit_code=15 elapsed=460.204s\n",
        encoding="utf-8",
    )
    summary_path.write_text(
        json.dumps(
            {
                "status": "guard_worker_exited_without_final_summary",
                "returncode": 15,
                "worker_returncode": 15,
                "worker_exit_signal": {"signal": 15, "name": "SIGTERM"},
                "recorded_at": "2026-07-08T23:48:21Z",
                "incident": {
                    "reason": "guard_worker_exited_without_final_summary",
                    "previous_status": "running",
                },
                "child_process": {
                    "pid": 40_132,
                    "command": [
                        sys.executable,
                        str(state.ROOT / "tools" / "memory_guard.py"),
                    ],
                },
            }
        ),
        encoding="utf-8",
    )
    conn = state._connect(db)
    scheduling._insert_run(
        conn,
        run_id="failed-run",
        logical_id="failed",
        reason="prove wrapper-terminalized memory_guard worker exits are classified",
        command=[sys.executable, "-c", "print('failed')"],
        cwd=state.ROOT,
        resource_family="python",
        contention_key="python-proof",
        scopes=["tools/proof_queue.py", "tools/memory_guard.py"],
        log_path=log_path,
        summary_json=summary_path,
    )
    state._insert_note(
        conn,
        run_id="failed-run",
        body="test: wrapper-terminalized memory_guard worker exit must classify",
        kind="submission",
        author="codex",
    )
    state._update_run(
        conn,
        "failed-run",
        status="failed",
        returncode=15,
        elapsed_s=460.204,
    )

    assert (
        cli.main(
            [
                "--db",
                str(db),
                "--logs-root",
                str(tmp_path / "runs"),
                "--repo-root",
                str(state.ROOT),
                "diagnose",
                "failed-run",
            ]
        )
        == 0
    )

    out = capsys.readouterr().out
    assert "memory-guard-worker-exit-without-final-summary" in out
    assert "worker_returncode=15" in out
    assert "worker_signal=SIGTERM" in out
    assert "previous_status=running" in out
    assert "child_process=memory_guard_child_process pid=40132" in out
    assert "last_log=proof_queue finished status=failed exit_code=15" in out
    assert "memory-guard-summary-incomplete" not in out
    assert "unclassified-failed-proof" not in out


def test_proof_queue_audit_treats_pruned_stale_incomplete_summary_as_warning(
    tmp_path: Path,
    capsys: pytest.CaptureFixture[str],
) -> None:
    db = tmp_path / "proof_queue.sqlite3"
    log_path = tmp_path / "stale.log"
    summary_path = tmp_path / "stale.memory_guard.json"
    log_path.write_text("proof_queue run_id=stale-run\n", encoding="utf-8")
    summary_path.write_text(
        json.dumps(
            {
                "status": "running",
                "returncode": None,
                "child_process": {"pid": 3210},
                "recorded_at": "2026-07-03T00:08:37Z",
            }
        ),
        encoding="utf-8",
    )
    conn = state._connect(db)
    scheduling._insert_run(
        conn,
        run_id="stale-run",
        logical_id="stale",
        reason="prove pruned stale rows remain visible without failing audit",
        command=[sys.executable, "-c", "print('stale')"],
        cwd=state.ROOT,
        resource_family="python",
        contention_key="python-proof",
        scopes=["tools/proof_queue.py"],
        log_path=log_path,
        summary_json=summary_path,
    )
    state._insert_note(
        conn,
        run_id="stale-run",
        body="test: stale row intentionally pruned after custody loss",
        kind="finding",
        author="codex",
    )
    state._update_run(conn, "stale-run", status="stale")

    assert (
        cli.main(
            [
                "--db",
                str(db),
                "--logs-root",
                str(tmp_path / "runs"),
                "--repo-root",
                str(state.ROOT),
                "audit",
                "--no-notebook-check",
            ]
        )
        == 0
    )
    out = capsys.readouterr().out
    assert "warning audit-memory-guard-summary-incomplete run=stale-run" in out
    assert "error audit-memory-guard-summary-incomplete run=stale-run" not in out


def test_proof_queue_prune_stale_preserves_live_launch_summary_only_row(
    monkeypatch: pytest.MonkeyPatch,
    tmp_path: Path,
    capsys: pytest.CaptureFixture[str],
) -> None:
    db = tmp_path / "proof_queue.sqlite3"
    log_path = tmp_path / "active.log"
    summary_path = tmp_path / "active.memory_guard.json"
    log_path.write_text(
        "proof_queue run_id=active-run\n done\n",
        encoding="utf-8",
    )
    stale = (
        time.time() - diagnostics_module.RUNNING_CHILD_MISSING_STALE_LOG_SECONDS - 5.0
    )
    os.utime(log_path, (stale, stale))
    summary_path.write_text(
        json.dumps(
            {
                "status": "running",
                "child_process": None,
                "returncode": None,
            }
        ),
        encoding="utf-8",
    )
    monkeypatch.setattr(custody, "_pid_alive", lambda pid: pid == 99_001)
    conn = state._connect(db)
    scheduling._insert_run(
        conn,
        run_id="active-run",
        logical_id="active",
        reason="prove launch-summary-only rows are diagnostic, not terminal",
        command=[sys.executable, "-c", "print('active')"],
        cwd=state.ROOT,
        resource_family="wasm",
        contention_key="wasm-build",
        scopes=["tools/proof_queue.py"],
        log_path=log_path,
        summary_json=summary_path,
    )
    state._update_run(
        conn,
        "active-run",
        status="running",
        guard_pid=99_001,
        started_at=state._utc_now(),
    )

    assert (
        cli.main(
            [
                "--db",
                str(db),
                "--logs-root",
                str(tmp_path / "runs"),
                "--repo-root",
                str(state.ROOT),
                "prune-stale",
            ]
        )
        == 0
    )

    out = capsys.readouterr().out
    assert "stale active-run" not in out
    assert "diagnosis=running-proof-launch-summary-stale" not in out
    assert "pruned=0" in out
    row = _rows(db)[0]
    assert row["status"] == "running"
    assert row["returncode"] is None

    assert (
        cli.main(
            [
                "--db",
                str(db),
                "--logs-root",
                str(tmp_path / "runs"),
                "--repo-root",
                str(state.ROOT),
                "diagnose",
                "active-run",
            ]
        )
        == 0
    )
    diag_out = capsys.readouterr().out
    assert "running-proof-launch-summary-stale" in diag_out
    assert str(summary_path) in diag_out


def test_proof_queue_prune_stale_reclaims_launch_summary_after_guard_exit(
    monkeypatch: pytest.MonkeyPatch,
    tmp_path: Path,
    capsys: pytest.CaptureFixture[str],
) -> None:
    db = tmp_path / "proof_queue.sqlite3"
    log_path = tmp_path / "active.log"
    summary_path = tmp_path / "active.memory_guard.json"
    log_path.write_text(
        "proof_queue run_id=active-run\n done\n",
        encoding="utf-8",
    )
    stale = (
        time.time() - diagnostics_module.RUNNING_CHILD_MISSING_STALE_LOG_SECONDS - 5.0
    )
    os.utime(log_path, (stale, stale))
    summary_path.write_text(
        json.dumps(
            {
                "status": "running",
                "child_process": None,
                "returncode": None,
            }
        ),
        encoding="utf-8",
    )
    monkeypatch.setattr(custody, "_pid_alive", lambda pid: False)
    conn = state._connect(db)
    scheduling._insert_run(
        conn,
        run_id="active-run",
        logical_id="active",
        reason="prove dead guard launch-summary rows are reclaimable",
        command=[sys.executable, "-c", "print('active')"],
        cwd=state.ROOT,
        resource_family="wasm",
        contention_key="wasm-build",
        scopes=["tools/proof_queue.py"],
        log_path=log_path,
        summary_json=summary_path,
    )
    state._update_run(
        conn,
        "active-run",
        status="running",
        guard_pid=99_001,
        started_at=state._utc_now(),
    )

    assert (
        cli.main(
            [
                "--db",
                str(db),
                "--logs-root",
                str(tmp_path / "runs"),
                "--repo-root",
                str(state.ROOT),
                "prune-stale",
            ]
        )
        == 0
    )

    out = capsys.readouterr().out
    assert "stale active-run" in out
    assert "diagnosis=running-proof-launch-summary-stale" in out
    assert "pruned=1" in out
    row = _rows(db)[0]
    assert row["status"] == "stale"
    assert row["returncode"] == custody.PROOF_QUEUE_STALE_EXIT_CODE


def test_proof_queue_prune_stale_terminalizes_dead_nested_guard_child(
    monkeypatch: pytest.MonkeyPatch,
    tmp_path: Path,
    capsys: pytest.CaptureFixture[str],
) -> None:
    db = tmp_path / "proof_queue.sqlite3"
    log_path = tmp_path / "active.log"
    summary_path = tmp_path / "active.memory_guard.json"
    guard_pid = 99_001
    child_pid = 99_101
    log_path.write_text(
        "proof_queue run_id=active-run\n"
        "memory_guard_command='python tools/memory_guard.py -- pytest tests'\n",
        encoding="utf-8",
    )
    stale = (
        time.time() - diagnostics_module.RUNNING_CHILD_MISSING_STALE_LOG_SECONDS - 5.0
    )
    os.utime(log_path, (stale, stale))
    summary_path.write_text(
        json.dumps(
            {
                "status": "child_running",
                "returncode": None,
                "child_process": {
                    "pid": child_pid,
                    "command": [
                        sys.executable,
                        str(state.ROOT / "tools" / "memory_guard.py"),
                    ],
                },
            }
        ),
        encoding="utf-8",
    )
    monkeypatch.setattr(custody, "_pid_alive", lambda pid: pid == guard_pid)
    conn = state._connect(db)
    scheduling._insert_run(
        conn,
        run_id="active-run",
        logical_id="active",
        reason="prove prune terminalizes dead nested guard child",
        command=[sys.executable, "-c", "print('active')"],
        cwd=state.ROOT,
        resource_family="python-tests",
        contention_key="proof-queue-dx",
        scopes=["tools/proof_queue.py"],
        log_path=log_path,
        summary_json=summary_path,
    )
    state._update_run(
        conn,
        "active-run",
        status="running",
        guard_pid=guard_pid,
        started_at=state._utc_now(),
    )

    assert (
        cli.main(
            [
                "--db",
                str(db),
                "--logs-root",
                str(tmp_path / "runs"),
                "--repo-root",
                str(state.ROOT),
                "prune-stale",
                "--run-id",
                "active-run",
            ]
        )
        == 0
    )

    out = capsys.readouterr().out
    assert "stale active-run" in out
    assert "diagnosis=running-proof-child-missing" in out
    assert f"child_pid={child_pid}" in out
    assert "pruned=1" in out
    row = _rows(db)[0]
    assert row["status"] == "stale"
    assert row["returncode"] == custody.PROOF_QUEUE_STALE_EXIT_CODE


def test_proof_queue_prune_stale_preserves_live_windows_child_runner_missing(
    monkeypatch: pytest.MonkeyPatch,
    tmp_path: Path,
    capsys: pytest.CaptureFixture[str],
) -> None:
    db = tmp_path / "proof_queue.sqlite3"
    log_path = tmp_path / "active.log"
    summary_path = tmp_path / "active.memory_guard.json"
    guard_pid = 99_001
    child_pid = 99_101
    log_path.write_text(
        "proof_queue run_id=active-run\n"
        "dx-build prime: still running elapsed=208s timeout=unbounded pid=18956\n",
        encoding="utf-8",
    )
    stale = (
        time.time() - diagnostics_module.RUNNING_CHILD_MISSING_STALE_LOG_SECONDS - 5.0
    )
    os.utime(log_path, (stale, stale))
    summary_path.write_text(
        json.dumps(
            {
                "status": "child_running",
                "returncode": None,
                "repro": {"host": {"platform": "win32"}},
                "child_process": {
                    "pid": child_pid,
                    "command": [
                        sys.executable,
                        str(state.ROOT / "tools" / "memory_guard.py"),
                    ],
                },
            }
        ),
        encoding="utf-8",
    )
    monkeypatch.setattr(custody, "_pid_alive", lambda pid: pid == guard_pid)
    conn = state._connect(db)
    scheduling._insert_run(
        conn,
        run_id="active-run",
        logical_id="active",
        reason="prove live Windows child-runner loss is diagnostic only",
        command=[sys.executable, "-c", "print('active')"],
        cwd=state.ROOT,
        resource_family="rust",
        contention_key="e2-build-wallclock",
        scopes=["tools/proof_queue.py"],
        log_path=log_path,
        summary_json=summary_path,
    )
    state._update_run(
        conn,
        "active-run",
        status="running",
        guard_pid=guard_pid,
        started_at=state._utc_now(),
    )

    assert (
        cli.main(
            [
                "--db",
                str(db),
                "--logs-root",
                str(tmp_path / "runs"),
                "--repo-root",
                str(state.ROOT),
                "prune-stale",
                "--run-id",
                "active-run",
            ]
        )
        == 0
    )

    out = capsys.readouterr().out
    assert "stale active-run" not in out
    assert "pruned=0" in out
    row = _rows(db)[0]
    assert row["status"] == "running"
    assert row["returncode"] is None

    assert (
        cli.main(
            [
                "--db",
                str(db),
                "--logs-root",
                str(tmp_path / "runs"),
                "--repo-root",
                str(state.ROOT),
                "diagnose",
                "active-run",
            ]
        )
        == 0
    )
    diagnose_out = capsys.readouterr().out
    assert "running-proof-windows-child-runner-missing" in diagnose_out
    assert "child_process=windows_memory_guard_child_runner" in diagnose_out
    assert "running-proof-child-missing" not in diagnose_out


def test_proof_queue_run_self_terminalizes_dead_nested_guard_child(
    monkeypatch: pytest.MonkeyPatch,
    tmp_path: Path,
    capsys: pytest.CaptureFixture[str],
) -> None:
    db = tmp_path / "proof_queue.sqlite3"
    logs = tmp_path / "runs"
    guard_pid = 91_001
    child_pid = 91_101
    popen_instances: list[object] = []

    class FakePopen:
        pid = guard_pid

        def __init__(self, command: list[str], **kwargs: object) -> None:
            self.command = command
            self.kwargs = kwargs
            self.returncode: int | None = None
            self.terminated = False
            self.killed = False
            summary_path = Path(command[command.index("--summary-json") + 1])
            summary_path.parent.mkdir(parents=True, exist_ok=True)
            summary_path.write_text(
                json.dumps(
                    {
                        "status": "child_running",
                        "returncode": None,
                        "child_process": {
                            "pid": child_pid,
                            "command": [
                                sys.executable,
                                str(state.ROOT / "tools" / "memory_guard.py"),
                            ],
                        },
                    }
                ),
                encoding="utf-8",
            )
            stdout = kwargs["stdout"]
            stdout.flush()
            os.utime(stdout.name, (time.time() - 1.0, time.time() - 1.0))
            popen_instances.append(self)

        def wait(self, timeout: float | None = None) -> int:
            if self.returncode is None:
                raise subprocess.TimeoutExpired(self.command, timeout)
            return self.returncode

        def poll(self) -> int | None:
            return self.returncode

        def terminate(self) -> None:
            self.terminated = True
            self.returncode = 15

        def kill(self) -> None:
            self.killed = True
            self.returncode = 9

    monkeypatch.setattr(custody, "Popen", FakePopen)
    monkeypatch.setattr(
        state,
        "_git_snapshot",
        lambda cwd: {
            "available": True,
            "head": "abc123",
            "dirty": False,
            "status": [],
        },
    )
    monkeypatch.setattr(custody, "_pid_alive", lambda pid: pid == guard_pid)
    monkeypatch.setattr(custody, "PROOF_QUEUE_ACTIVE_POLL_SECONDS", 0.01)
    monkeypatch.setattr(
        custody,
        "PROOF_QUEUE_STALE_TERMINATE_GRACE_SECONDS",
        0.01,
    )
    monkeypatch.setattr(
        diagnostics_module, "RUNNING_CHILD_MISSING_STALE_LOG_SECONDS", 0.0
    )

    rc = cli.main(
        [
            "--db",
            str(db),
            "--logs-root",
            str(logs),
            "--repo-root",
            str(state.ROOT),
            "exec",
            "--id",
            "self-stale",
            "--reason",
            "prove runner self-terminalizes dead guard child",
            "--resource-family",
            "python-tests",
            "--contention-key",
            "proof-queue-dx:self-stale",
            "--scope",
            "tools/proof_queue.py",
            "--",
            sys.executable,
            "-c",
            "print('unreachable')",
        ]
    )

    assert rc == custody.PROOF_QUEUE_STALE_EXIT_CODE
    out = capsys.readouterr().out
    assert "stale " in out
    assert f"rc={custody.PROOF_QUEUE_STALE_EXIT_CODE}" in out
    rows = _rows(db)
    assert rows[0]["status"] == "stale"
    assert rows[0]["returncode"] == custody.PROOF_QUEUE_STALE_EXIT_CODE
    assert popen_instances
    fake_proc = popen_instances[0]
    assert fake_proc.terminated
    assert not fake_proc.killed
    log_text = Path(rows[0]["log_path"]).read_text(encoding="utf-8")
    assert "proof_queue stale-running terminalization" in log_text
    assert "diagnosis=running-proof-child-missing" in log_text
    assert f"child_pid={child_pid}" in log_text
    assert (
        f"proof_queue finished status=stale "
        f"exit_code={custody.PROOF_QUEUE_STALE_EXIT_CODE}" in log_text
    )


def test_queue_process_fake_is_scoped_to_custody_constructor(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    global_constructor = subprocess.Popen

    class FakePopen:
        pass

    monkeypatch.setattr(custody, "Popen", FakePopen)

    assert custody.Popen is FakePopen
    assert subprocess.Popen is global_constructor


def test_proof_queue_run_does_not_self_terminalize_windows_child_runner_missing(
    monkeypatch: pytest.MonkeyPatch,
    tmp_path: Path,
    capsys: pytest.CaptureFixture[str],
) -> None:
    db = tmp_path / "proof_queue.sqlite3"
    logs = tmp_path / "runs"
    guard_pid = 91_001
    child_pid = 91_101
    popen_instances: list[object] = []

    class FakePopen:
        pid = guard_pid

        def __init__(self, command: list[str], **kwargs: object) -> None:
            self.command = command
            self.kwargs = kwargs
            self.returncode: int | None = None
            self.terminated = False
            self.wait_count = 0
            summary_path = Path(command[command.index("--summary-json") + 1])
            summary_path.parent.mkdir(parents=True, exist_ok=True)
            summary_path.write_text(
                json.dumps(
                    {
                        "status": "child_running",
                        "returncode": None,
                        "repro": {"host": {"platform": "win32"}},
                        "child_process": {
                            "pid": child_pid,
                            "command": [
                                sys.executable,
                                str(state.ROOT / "tools" / "memory_guard.py"),
                            ],
                        },
                    }
                ),
                encoding="utf-8",
            )
            stdout = kwargs["stdout"]
            stdout.flush()
            os.utime(stdout.name, (time.time() - 1.0, time.time() - 1.0))
            popen_instances.append(self)

        def wait(self, timeout: float | None = None) -> int:
            self.wait_count += 1
            if self.wait_count == 1:
                raise subprocess.TimeoutExpired(self.command, timeout)
            self.returncode = 0
            return self.returncode

        def poll(self) -> int | None:
            return self.returncode

        def terminate(self) -> None:
            self.terminated = True
            self.returncode = 15

    monkeypatch.setattr(custody, "Popen", FakePopen)
    monkeypatch.setattr(
        state,
        "_git_snapshot",
        lambda cwd: {
            "available": True,
            "head": "abc123",
            "dirty": False,
            "status": [],
        },
    )
    monkeypatch.setattr(custody, "_pid_alive", lambda pid: pid == guard_pid)
    monkeypatch.setattr(custody, "PROOF_QUEUE_ACTIVE_POLL_SECONDS", 0.01)
    monkeypatch.setattr(
        diagnostics_module, "RUNNING_CHILD_MISSING_STALE_LOG_SECONDS", 0.0
    )

    rc = cli.main(
        [
            "--db",
            str(db),
            "--logs-root",
            str(logs),
            "--repo-root",
            str(state.ROOT),
            "exec",
            "--id",
            "windows-child-runner",
            "--reason",
            "prove live Windows child-runner loss is not terminal",
            "--resource-family",
            "python-tests",
            "--contention-key",
            "proof-queue-dx:windows-child-runner",
            "--scope",
            "tools/proof_queue.py",
            "--",
            sys.executable,
            "-c",
            "print('eventual pass')",
        ]
    )

    assert rc == 0
    out = capsys.readouterr().out
    assert "passed " in out
    rows = _rows(db)
    assert rows[0]["status"] == "passed"
    assert rows[0]["returncode"] == 0
    assert popen_instances
    fake_proc = popen_instances[0]
    assert fake_proc.wait_count == 2
    assert not fake_proc.terminated
    log_text = Path(rows[0]["log_path"]).read_text(encoding="utf-8")
    assert "proof_queue stale-running terminalization" not in log_text
    assert "running-proof-windows-child-runner-missing" not in log_text
    assert "proof_queue finished status=passed exit_code=0" in log_text


def test_proof_queue_run_does_not_self_terminalize_launch_summary_only(
    monkeypatch: pytest.MonkeyPatch,
    tmp_path: Path,
    capsys: pytest.CaptureFixture[str],
) -> None:
    db = tmp_path / "proof_queue.sqlite3"
    logs = tmp_path / "runs"
    guard_pid = 92_001
    popen_instances: list[object] = []

    class FakePopen:
        pid = guard_pid

        def __init__(self, command: list[str], **kwargs: object) -> None:
            self.command = command
            self.kwargs = kwargs
            self.returncode: int | None = None
            self.terminated = False
            self.wait_count = 0
            summary_path = Path(command[command.index("--summary-json") + 1])
            summary_path.parent.mkdir(parents=True, exist_ok=True)
            summary_path.write_text(
                json.dumps(
                    {
                        "status": "running",
                        "child_process": None,
                        "returncode": None,
                        "recorded_at": "2026-07-05T20:25:05Z",
                    }
                ),
                encoding="utf-8",
            )
            stdout = kwargs["stdout"]
            stdout.flush()
            old = time.time() - 60.0
            os.utime(stdout.name, (old, old))
            popen_instances.append(self)

        def wait(self, timeout: float | None = None) -> int:
            self.wait_count += 1
            if self.wait_count == 1:
                raise subprocess.TimeoutExpired(self.command, timeout)
            self.returncode = 0
            return self.returncode

        def poll(self) -> int | None:
            return self.returncode

        def terminate(self) -> None:
            self.terminated = True
            self.returncode = 15

    monkeypatch.setattr(custody, "Popen", FakePopen)
    monkeypatch.setattr(
        state,
        "_git_snapshot",
        lambda cwd: {
            "available": True,
            "head": "abc123",
            "dirty": False,
            "status": [],
        },
    )
    monkeypatch.setattr(custody, "_pid_alive", lambda pid: pid == guard_pid)
    monkeypatch.setattr(custody, "PROOF_QUEUE_ACTIVE_POLL_SECONDS", 0.01)
    monkeypatch.setattr(
        diagnostics_module, "RUNNING_CHILD_MISSING_STALE_LOG_SECONDS", 0.0
    )

    rc = cli.main(
        [
            "--db",
            str(db),
            "--logs-root",
            str(logs),
            "--repo-root",
            str(state.ROOT),
            "exec",
            "--id",
            "launch-summary-only",
            "--reason",
            "prove runner does not kill launch-summary-only guard",
            "--resource-family",
            "python-tests",
            "--contention-key",
            "proof-queue-dx:launch-summary-only",
            "--scope",
            "tools/proof_queue.py",
            "--",
            sys.executable,
            "-c",
            "print('eventual pass')",
        ]
    )

    assert rc == 0
    out = capsys.readouterr().out
    assert "passed " in out
    rows = _rows(db)
    assert rows[0]["status"] == "passed"
    assert rows[0]["returncode"] == 0
    assert popen_instances
    fake_proc = popen_instances[0]
    assert fake_proc.wait_count == 2
    assert not fake_proc.terminated
    log_text = Path(rows[0]["log_path"]).read_text(encoding="utf-8")
    assert "proof_queue stale-running terminalization" not in log_text
    assert "proof_queue finished status=passed exit_code=0" in log_text


def test_proof_queue_prune_stale_run_id_preserves_unselected_active_rows(
    monkeypatch: pytest.MonkeyPatch,
    tmp_path: Path,
    capsys: pytest.CaptureFixture[str],
) -> None:
    db = tmp_path / "proof_queue.sqlite3"
    stale_mtime = (
        time.time() - diagnostics_module.RUNNING_CHILD_MISSING_STALE_LOG_SECONDS - 5.0
    )
    conn = state._connect(db)
    for run_id, guard_pid in (("target-run", 99_001), ("sibling-run", 99_002)):
        log_path = tmp_path / f"{run_id}.log"
        summary_path = tmp_path / f"{run_id}.memory_guard.json"
        log_path.write_text(f"proof_queue run_id={run_id}\n", encoding="utf-8")
        os.utime(log_path, (stale_mtime, stale_mtime))
        summary_path.write_text(
            json.dumps(
                {
                    "status": "running",
                    "child_process": None,
                    "returncode": None,
                }
            ),
            encoding="utf-8",
        )
        scheduling._insert_run(
            conn,
            run_id=run_id,
            logical_id=run_id,
            reason="prove targeted stale pruning",
            command=[sys.executable, "-c", "print('active')"],
            cwd=state.ROOT,
            resource_family="python-tests",
            # Distinct keys per run: at most one RUNNING row may share a
            # contention key, and this test needs both concurrently running to
            # prove targeted pruning preserves the unselected sibling.
            contention_key=f"proof-queue-dx-{run_id}",
            scopes=["tools/proof_queue.py"],
            log_path=log_path,
            summary_json=summary_path,
        )
        state._update_run(
            conn,
            run_id,
            status="running",
            guard_pid=guard_pid,
            started_at=state._utc_now(),
        )
    # The selected target's guard has exited; the unselected sibling still owns
    # live custody. This keeps the test focused on --run-id scoping now that a
    # launch-summary-only diagnostic is not terminal while its guard is live.
    monkeypatch.setattr(custody, "_pid_alive", lambda pid: pid == 99_002)

    assert (
        cli.main(
            [
                "--db",
                str(db),
                "--logs-root",
                str(tmp_path / "runs"),
                "--repo-root",
                str(state.ROOT),
                "prune-stale",
                "--run-id",
                "target-run",
            ]
        )
        == 0
    )

    out = capsys.readouterr().out
    assert "stale target-run" in out
    assert "diagnosis=running-proof-launch-summary-stale" in out
    assert str(tmp_path / "target-run.memory_guard.json") in out
    assert str(tmp_path / "target-run.log") in out
    assert "sibling-run" not in out
    assert "pruned=1" in out
    rows = {row["run_id"]: row for row in _rows(db)}
    statuses = {run_id: row["status"] for run_id, row in rows.items()}
    assert statuses == {"target-run": "stale", "sibling-run": "running"}
    assert rows["target-run"]["returncode"] == custody.PROOF_QUEUE_STALE_EXIT_CODE
    assert rows["sibling-run"]["returncode"] is None


def test_proof_queue_prune_stale_run_id_canonicalizes_selected_stale_row(
    tmp_path: Path,
    capsys: pytest.CaptureFixture[str],
) -> None:
    db = tmp_path / "proof_queue.sqlite3"
    log_path = tmp_path / "stale.log"
    summary_path = tmp_path / "stale.memory_guard.json"
    log_path.write_text(
        "proof_queue run_id=stale-run\n"
        "proof_queue finished status=stale exit_code=? elapsed=17.0s\n",
        encoding="utf-8",
    )
    summary_path.write_text(
        json.dumps(
            {
                "status": "running",
                "returncode": None,
                "child_process": None,
                "recorded_at": "2026-07-03T00:08:37Z",
            }
        ),
        encoding="utf-8",
    )
    conn = state._connect(db)
    scheduling._insert_run(
        conn,
        run_id="stale-run",
        logical_id="stale",
        reason="prove selected stale rows get canonical return codes",
        command=[sys.executable, "-c", "print('stale')"],
        cwd=state.ROOT,
        resource_family="python-tests",
        contention_key="proof-queue-dx",
        scopes=["tools/proof_queue.py"],
        log_path=log_path,
        summary_json=summary_path,
    )
    state._update_run(conn, "stale-run", status="stale")

    assert (
        cli.main(
            [
                "--db",
                str(db),
                "--logs-root",
                str(tmp_path / "runs"),
                "--repo-root",
                str(state.ROOT),
                "prune-stale",
                "--run-id",
                "stale-run",
            ]
        )
        == 0
    )

    out = capsys.readouterr().out
    assert "stale stale-run" in out
    assert "memory-guard-summary-incomplete" in out
    assert f"returncode={custody.PROOF_QUEUE_STALE_EXIT_CODE}" in out
    assert "pruned=1" in out
    row = _rows(db)[0]
    assert row["status"] == "stale"
    assert row["returncode"] == custody.PROOF_QUEUE_STALE_EXIT_CODE


def test_proof_queue_wasm_rows_ensure_rust_target_before_run(
    monkeypatch: pytest.MonkeyPatch, tmp_path: Path
) -> None:
    db = tmp_path / "proof_queue.sqlite3"
    logs = tmp_path / "runs"
    calls: list[tuple[str, Path | None]] = []
    required_targets = ("wasm32-wasip1", "wasm32-unknown-unknown")

    def fake_ensure(
        target: str, warnings: list[str], *, root: Path | None = None
    ) -> bool:
        del warnings
        calls.append((target, root))
        return True

    fake_toolchain = SimpleNamespace(
        RustToolchainContractError=RuntimeError,
        rust_toolchain_contract=lambda repo_root: SimpleNamespace(
            required_wasm_targets=required_targets
        ),
        ensure_rustup_target=fake_ensure,
    )
    monkeypatch.setattr(policy, "_load_wasm_toolchain", lambda: fake_toolchain)

    rc = cli.main(
        [
            "--db",
            str(db),
            "--logs-root",
            str(logs),
            "--repo-root",
            str(state.ROOT),
            "exec",
            "--id",
            "wasm-preflight",
            "--reason",
            "prove wasm target preflight",
            "--resource-family",
            "wasm-browser",
            "--contention-key",
            "wasm:preflight",
            "--",
            sys.executable,
            "-c",
            "print('ran')",
        ]
    )

    assert rc == 0
    assert calls == [(target, state.ROOT) for target in required_targets]
    assert ("wasm32-wasip1", state.ROOT) in calls
    rows = _rows(db)
    assert rows[0]["status"] == "passed"
    assert "ran" in Path(rows[0]["log_path"]).read_text(encoding="utf-8")


def test_proof_queue_wasm_preflight_fails_before_command(
    monkeypatch: pytest.MonkeyPatch, tmp_path: Path
) -> None:
    db = tmp_path / "proof_queue.sqlite3"
    logs = tmp_path / "runs"
    required_targets = ("wasm32-wasip1",)

    def fake_ensure(
        target: str, warnings: list[str], *, root: Path | None = None
    ) -> bool:
        del root
        warnings.append(f"missing {target}")
        return False

    fake_toolchain = SimpleNamespace(
        RustToolchainContractError=RuntimeError,
        rust_toolchain_contract=lambda repo_root: SimpleNamespace(
            required_wasm_targets=required_targets
        ),
        ensure_rustup_target=fake_ensure,
    )
    monkeypatch.setattr(policy, "_load_wasm_toolchain", lambda: fake_toolchain)

    rc = cli.main(
        [
            "--db",
            str(db),
            "--logs-root",
            str(logs),
            "--repo-root",
            str(state.ROOT),
            "exec",
            "--id",
            "wasm-preflight-fail",
            "--reason",
            "prove wasm target preflight fails closed",
            "--resource-family",
            "wasm-browser",
            "--contention-key",
            "wasm:preflight-fail",
            "--",
            sys.executable,
            "-c",
            "print('should-not-run')",
        ]
    )

    rows = _rows(db)
    assert rc == 2
    assert rows[0]["status"] == "failed"
    assert rows[0]["returncode"] == 2
    log_text = Path(rows[0]["log_path"]).read_text(encoding="utf-8")
    assert "proof queue toolchain preflight failed" in log_text
    assert "missing wasm32-wasip1" in log_text
    assert "should-not-run" in log_text


def test_proof_queue_run_id_executes_only_selected_queued_row(tmp_path: Path) -> None:
    db = tmp_path / "proof_queue.sqlite3"
    logs = tmp_path / "runs"
    conn = state._connect(db)
    for run_id, marker in (("queued-a", "A"), ("queued-b", "B")):
        scheduling._insert_run(
            conn,
            run_id=run_id,
            logical_id=run_id,
            reason=f"run {marker}",
            command=[sys.executable, "-c", f"print('{marker}')"],
            cwd=state.ROOT,
            resource_family="python",
            contention_key=f"python:{marker}",
            scopes=[],
            log_path=logs / f"{run_id}.log",
            summary_json=logs / f"{run_id}.memory_guard.json",
        )

    rc = cli.main(
        [
            "--db",
            str(db),
            "--logs-root",
            str(logs),
            "--repo-root",
            str(state.ROOT),
            "run",
            "--run-id",
            "queued-b",
        ]
    )

    rows = {row["run_id"]: row for row in _rows(db)}
    assert rc == 0
    assert rows["queued-a"]["status"] == "queued"
    assert rows["queued-b"]["status"] == "passed"
    assert "B" in (logs / "queued-b.log").read_text(encoding="utf-8")


def test_proof_queue_run_id_executes_selected_dispatched_row(tmp_path: Path) -> None:
    db = tmp_path / "proof_queue.sqlite3"
    logs = tmp_path / "runs"
    conn = state._connect(db)
    for run_id, marker in (("queued-a", "A"), ("dispatched-b", "B")):
        scheduling._insert_run(
            conn,
            run_id=run_id,
            logical_id=run_id,
            reason=f"run {marker}",
            command=[sys.executable, "-c", f"print('{marker}')"],
            cwd=state.ROOT,
            resource_family="python",
            contention_key=f"python:{marker}",
            scopes=[],
            log_path=logs / f"{run_id}.log",
            summary_json=logs / f"{run_id}.memory_guard.json",
        )
    state._update_run(
        conn,
        "dispatched-b",
        status="dispatched",
        started_at=state._utc_now(),
    )

    rc = cli.main(
        [
            "--db",
            str(db),
            "--logs-root",
            str(logs),
            "--repo-root",
            str(state.ROOT),
            "run",
            "--run-id",
            "dispatched-b",
        ]
    )

    rows = {row["run_id"]: row for row in _rows(db)}
    assert rc == 0
    assert rows["queued-a"]["status"] == "queued"
    assert rows["dispatched-b"]["status"] == "passed"
    assert "B" in (logs / "dispatched-b.log").read_text(encoding="utf-8")


def test_proof_queue_run_id_can_detach_existing_queued_row(
    monkeypatch: pytest.MonkeyPatch,
    tmp_path: Path,
    capsys: pytest.CaptureFixture[str],
) -> None:
    db = tmp_path / "proof_queue.sqlite3"
    logs = tmp_path / "runs"
    conn = state._connect(db)
    for run_id, marker in (("queued-a", "A"), ("queued-b", "B")):
        scheduling._insert_run(
            conn,
            run_id=run_id,
            logical_id=run_id,
            reason=f"run {marker}",
            command=[sys.executable, "-c", f"print('{marker}')"],
            cwd=state.ROOT,
            resource_family="python",
            contention_key=f"python:{marker}",
            scopes=[],
            log_path=logs / f"{run_id}.log",
            summary_json=logs / f"{run_id}.memory_guard.json",
        )
    launched: dict[str, object] = {}

    def fake_launch(args: object, *, run_id: str, timeout: float) -> tuple[int, Path]:
        launched["run_id"] = run_id
        launched["timeout"] = timeout
        return 12345, logs / f"{run_id}.runner.log"

    monkeypatch.setattr(custody, "_launch_detached_runner", fake_launch)

    rc = cli.main(
        [
            "--db",
            str(db),
            "--logs-root",
            str(logs),
            "--repo-root",
            str(state.ROOT),
            "run",
            "--run-id",
            "queued-b",
            "--timeout",
            "42",
            "--detach",
        ]
    )

    rows = {row["run_id"]: row for row in _rows(db)}
    stdout = capsys.readouterr().out
    assert rc == 0
    assert launched == {"run_id": "queued-b", "timeout": 42.0}
    assert rows["queued-a"]["status"] == "queued"
    assert rows["queued-b"]["status"] == "dispatched"
    log_text = (logs / "queued-b.log").read_text(encoding="utf-8")
    assert "status=dispatched" in log_text
    assert "runner_pid=12345" in log_text
    assert "detached queued-b runner_pid=12345" in stdout
    assert f"runner_log: {logs / 'queued-b.runner.log'}" in stdout


def test_proof_queue_run_detach_respects_queue_size_and_contention_keys(
    monkeypatch: pytest.MonkeyPatch,
    tmp_path: Path,
) -> None:
    db = tmp_path / "proof_queue.sqlite3"
    logs = tmp_path / "runs"
    conn = state._connect(db)
    for run_id, key in (
        ("queued-a", "python:a"),
        ("queued-a-duplicate", "python:a"),
        ("queued-b", "python:b"),
        ("queued-c", "python:c"),
    ):
        scheduling._insert_run(
            conn,
            run_id=run_id,
            logical_id=run_id,
            reason=run_id,
            command=[sys.executable, "-c", f"print({run_id!r})"],
            cwd=state.ROOT,
            resource_family="python",
            contention_key=key,
            scopes=[],
            log_path=logs / f"{run_id}.log",
            summary_json=logs / f"{run_id}.memory_guard.json",
        )
    launched: list[str] = []

    def fake_launch(args: object, *, run_id: str, timeout: float) -> tuple[int, Path]:
        del args, timeout
        launched.append(run_id)
        return 12345, logs / f"{run_id}.runner.log"

    monkeypatch.setattr(custody, "_launch_detached_runner", fake_launch)

    rc = cli.main(
        [
            "--db",
            str(db),
            "--logs-root",
            str(logs),
            "--repo-root",
            str(state.ROOT),
            "run",
            "--detach",
            "--queue-size",
            "3",
        ]
    )

    rows = {row["run_id"]: row for row in _rows(db)}
    assert rc == 0
    assert launched == ["queued-a", "queued-b", "queued-c"]
    assert rows["queued-a"]["status"] == "dispatched"
    assert rows["queued-a-duplicate"]["status"] == "queued"
    assert rows["queued-b"]["status"] == "dispatched"
    assert rows["queued-c"]["status"] == "dispatched"


def test_proof_queue_run_detach_serializes_compiler_build_resource_mutex(
    monkeypatch: pytest.MonkeyPatch,
    tmp_path: Path,
    capsys: pytest.CaptureFixture[str],
) -> None:
    db = tmp_path / "proof_queue.sqlite3"
    logs = tmp_path / "runs"
    conn = state._connect(db)
    for run_id, resource_family, key in (
        ("queued-wasm", "wasm-browser", "wasm:pact-witness"),
        ("queued-native", "native-build", "native:molt-runtime"),
        ("queued-rust", "rust", "cargo:molt-runtime"),
        ("queued-python", "python", "python:light"),
    ):
        scheduling._insert_run(
            conn,
            run_id=run_id,
            logical_id=run_id,
            reason=run_id,
            command=[sys.executable, "-c", f"print({run_id!r})"],
            cwd=state.ROOT,
            resource_family=resource_family,
            contention_key=key,
            scopes=[],
            log_path=logs / f"{run_id}.log",
            summary_json=logs / f"{run_id}.memory_guard.json",
        )
    launched: list[str] = []

    def fake_launch(args: object, *, run_id: str, timeout: float) -> tuple[int, Path]:
        del args, timeout
        launched.append(run_id)
        return 12345, logs / f"{run_id}.runner.log"

    monkeypatch.setattr(custody, "_launch_detached_runner", fake_launch)

    rc = cli.main(
        [
            "--db",
            str(db),
            "--logs-root",
            str(logs),
            "--repo-root",
            str(state.ROOT),
            "run",
            "--detach",
            "--queue-size",
            "3",
        ]
    )

    rows = {row["run_id"]: row for row in _rows(db)}
    output = capsys.readouterr().out
    assert rc == 0
    assert launched == ["queued-wasm", "queued-python"]
    assert rows["queued-wasm"]["status"] == "dispatched"
    assert rows["queued-native"]["status"] == "queued"
    assert rows["queued-rust"]["status"] == "queued"
    assert rows["queued-python"]["status"] == "dispatched"
    assert "waiting queued-native resource_mutex=compiler-build-resource" in output
    assert "waiting queued-rust resource_mutex=compiler-build-resource" in output


def test_proof_queue_run_jobs_alias_limits_detached_rows(
    monkeypatch: pytest.MonkeyPatch,
    tmp_path: Path,
) -> None:
    db = tmp_path / "proof_queue.sqlite3"
    logs = tmp_path / "runs"
    conn = state._connect(db)
    for run_id in ("queued-a", "queued-b", "queued-c"):
        scheduling._insert_run(
            conn,
            run_id=run_id,
            logical_id=run_id,
            reason=run_id,
            command=[sys.executable, "-c", f"print({run_id!r})"],
            cwd=state.ROOT,
            resource_family="python",
            contention_key=f"python:{run_id}",
            scopes=[],
            log_path=logs / f"{run_id}.log",
            summary_json=logs / f"{run_id}.memory_guard.json",
        )
    launched: list[str] = []

    def fake_launch(args: object, *, run_id: str, timeout: float) -> tuple[int, Path]:
        del args, timeout
        launched.append(run_id)
        return 12345, logs / f"{run_id}.runner.log"

    monkeypatch.setattr(custody, "_launch_detached_runner", fake_launch)

    rc = cli.main(
        [
            "--db",
            str(db),
            "--logs-root",
            str(logs),
            "--repo-root",
            str(state.ROOT),
            "run",
            "--detach",
            "--queue-size",
            "3",
            "--jobs",
            "2",
        ]
    )

    rows = {row["run_id"]: row for row in _rows(db)}
    assert rc == 0
    assert launched == ["queued-a", "queued-b"]
    assert rows["queued-a"]["status"] == "dispatched"
    assert rows["queued-b"]["status"] == "dispatched"
    assert rows["queued-c"]["status"] == "queued"


def test_proof_queue_run_detach_counts_existing_active_rows(
    monkeypatch: pytest.MonkeyPatch,
    tmp_path: Path,
) -> None:
    db = tmp_path / "proof_queue.sqlite3"
    logs = tmp_path / "runs"
    conn = state._connect(db)
    scheduling._insert_run(
        conn,
        run_id="already-running",
        logical_id="already-running",
        reason="already running",
        command=[sys.executable, "-c", "print('running')"],
        cwd=state.ROOT,
        resource_family="python",
        contention_key="python:running",
        scopes=[],
        log_path=logs / "already-running.log",
        summary_json=logs / "already-running.memory_guard.json",
    )
    state._update_run(
        conn,
        "already-running",
        status="running",
        started_at=state._utc_now(),
    )
    for run_id in ("queued-a", "queued-b"):
        scheduling._insert_run(
            conn,
            run_id=run_id,
            logical_id=run_id,
            reason=run_id,
            command=[sys.executable, "-c", f"print({run_id!r})"],
            cwd=state.ROOT,
            resource_family="python",
            contention_key=f"python:{run_id}",
            scopes=[],
            log_path=logs / f"{run_id}.log",
            summary_json=logs / f"{run_id}.memory_guard.json",
        )
    launched: list[str] = []

    def fake_launch(args: object, *, run_id: str, timeout: float) -> tuple[int, Path]:
        del args, timeout
        launched.append(run_id)
        return 12345, logs / f"{run_id}.runner.log"

    monkeypatch.setattr(custody, "_launch_detached_runner", fake_launch)

    rc = cli.main(
        [
            "--db",
            str(db),
            "--logs-root",
            str(logs),
            "--repo-root",
            str(state.ROOT),
            "run",
            "--detach",
            "--queue-size",
            "2",
        ]
    )

    rows = {row["run_id"]: row for row in _rows(db)}
    assert rc == 0
    assert launched == ["queued-a"]
    assert rows["already-running"]["status"] == "running"
    assert rows["queued-a"]["status"] == "dispatched"
    assert rows["queued-b"]["status"] == "queued"


def test_proof_queue_run_detach_does_not_launch_when_capacity_full(
    monkeypatch: pytest.MonkeyPatch,
    tmp_path: Path,
    capsys: pytest.CaptureFixture[str],
) -> None:
    db = tmp_path / "proof_queue.sqlite3"
    logs = tmp_path / "runs"
    conn = state._connect(db)
    scheduling._insert_run(
        conn,
        run_id="already-dispatched",
        logical_id="already-dispatched",
        reason="already dispatched",
        command=[sys.executable, "-c", "print('dispatched')"],
        cwd=state.ROOT,
        resource_family="python",
        contention_key="python:active",
        scopes=[],
        log_path=logs / "already-dispatched.log",
        summary_json=logs / "already-dispatched.memory_guard.json",
    )
    state._update_run(
        conn,
        "already-dispatched",
        status="dispatched",
        started_at=state._utc_now(),
    )
    scheduling._insert_run(
        conn,
        run_id="queued-a",
        logical_id="queued-a",
        reason="queued-a",
        command=[sys.executable, "-c", "print('queued')"],
        cwd=state.ROOT,
        resource_family="python",
        contention_key="python:queued-a",
        scopes=[],
        log_path=logs / "queued-a.log",
        summary_json=logs / "queued-a.memory_guard.json",
    )
    monkeypatch.setattr(
        custody,
        "_launch_detached_runner",
        lambda *args, **kwargs: pytest.fail("capacity-full queue must not launch"),
    )

    rc = cli.main(
        [
            "--db",
            str(db),
            "--logs-root",
            str(logs),
            "--repo-root",
            str(state.ROOT),
            "run",
            "--detach",
            "--queue-size",
            "1",
        ]
    )

    rows = {row["run_id"]: row for row in _rows(db)}
    assert rc == 0
    assert rows["already-dispatched"]["status"] == "dispatched"
    assert rows["queued-a"]["status"] == "queued"
    assert "queue capacity full active=1 queue_size=1" in capsys.readouterr().out


def test_proof_queue_exec_detach_obeys_queue_size_env(
    monkeypatch: pytest.MonkeyPatch,
    tmp_path: Path,
    capsys: pytest.CaptureFixture[str],
) -> None:
    db = tmp_path / "proof_queue.sqlite3"
    logs = tmp_path / "runs"
    conn = state._connect(db)
    scheduling._insert_run(
        conn,
        run_id="already-running",
        logical_id="already-running",
        reason="already running",
        command=[sys.executable, "-c", "print('running')"],
        cwd=state.ROOT,
        resource_family="python",
        contention_key="python:active",
        scopes=[],
        log_path=logs / "already-running.log",
        summary_json=logs / "already-running.memory_guard.json",
    )
    state._update_run(
        conn,
        "already-running",
        status="running",
        started_at=state._utc_now(),
    )
    monkeypatch.setenv(state.PROOF_QUEUE_SIZE_ENV, "1")
    monkeypatch.setattr(
        custody,
        "_launch_detached_runner",
        lambda *args, **kwargs: pytest.fail("env capacity must block detach launch"),
    )

    rc = cli.main(
        [
            "--db",
            str(db),
            "--logs-root",
            str(logs),
            "--repo-root",
            str(state.ROOT),
            "exec",
            "--id",
            "blocked-detach",
            "--reason",
            "prove detached submissions obey queue capacity",
            "--resource-family",
            "python",
            "--contention-key",
            "python:new",
            "--note",
            "test: detached submission must remain queued when capacity is full",
            "--detach",
            "--",
            sys.executable,
            "-c",
            "print('blocked')",
        ]
    )

    rows = {row["run_id"]: row for row in _rows(db)}
    queued = [row for row in rows.values() if row["logical_id"] == "blocked-detach"]
    assert rc == 0
    assert len(queued) == 1
    assert queued[0]["status"] == "queued"
    assert "queue capacity full active=1 queue_size=1" in capsys.readouterr().out


def test_proof_queue_rejects_invalid_queue_size_env(
    monkeypatch: pytest.MonkeyPatch, tmp_path: Path
) -> None:
    monkeypatch.setenv(state.PROOF_QUEUE_SIZE_ENV, "0")
    with pytest.raises(SystemExit, match=state.PROOF_QUEUE_SIZE_ENV):
        cli.main(
            [
                "--db",
                str(tmp_path / "proof_queue.sqlite3"),
                "--logs-root",
                str(tmp_path / "runs"),
                "--repo-root",
                str(state.ROOT),
                "run",
                "--detach",
            ]
        )


def test_proof_queue_defaults_uv_link_mode_to_copy(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    monkeypatch.delenv("UV_LINK_MODE", raising=False)

    custody._normalize_queue_process_environment()

    assert os.environ["UV_LINK_MODE"] == "copy"


def test_proof_queue_preserves_operator_uv_link_mode(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    monkeypatch.setenv("UV_LINK_MODE", "hardlink")

    custody._normalize_queue_process_environment()

    assert os.environ["UV_LINK_MODE"] == "hardlink"


def test_proof_queue_prune_stale_preserves_fresh_dispatched_row(
    tmp_path: Path,
) -> None:
    db = tmp_path / "proof_queue.sqlite3"
    logs = tmp_path / "runs"
    conn = state._connect(db)
    scheduling._insert_run(
        conn,
        run_id="just-dispatched",
        logical_id="just-dispatched",
        reason="dispatch grace",
        command=[sys.executable, "-c", "print('dispatch')"],
        cwd=state.ROOT,
        resource_family="python",
        contention_key="python:dispatch",
        scopes=[],
        log_path=logs / "just-dispatched.log",
        summary_json=logs / "just-dispatched.memory_guard.json",
    )
    state._update_run(
        conn,
        "just-dispatched",
        status="dispatched",
        started_at=state._utc_now(),
    )

    assert (
        cli.main(
            [
                "--db",
                str(db),
                "--logs-root",
                str(logs),
                "--repo-root",
                str(state.ROOT),
                "prune-stale",
                "--run-id",
                "just-dispatched",
            ]
        )
        == 0
    )
    assert _rows(db)[0]["status"] == "dispatched"


def test_proof_queue_prune_stale_reclaims_expired_dispatched_row(
    tmp_path: Path,
    capsys: pytest.CaptureFixture[str],
) -> None:
    db = tmp_path / "proof_queue.sqlite3"
    logs = tmp_path / "runs"
    conn = state._connect(db)
    scheduling._insert_run(
        conn,
        run_id="expired-dispatch",
        logical_id="expired-dispatch",
        reason="expired dispatch",
        command=[sys.executable, "-c", "print('dispatch')"],
        cwd=state.ROOT,
        resource_family="python",
        contention_key="python:dispatch",
        scopes=[],
        log_path=logs / "expired-dispatch.log",
        summary_json=logs / "expired-dispatch.memory_guard.json",
    )
    state._update_run(
        conn,
        "expired-dispatch",
        status="dispatched",
        started_at="2000-01-01T00:00:00+00:00",
    )

    assert (
        cli.main(
            [
                "--db",
                str(db),
                "--logs-root",
                str(logs),
                "--repo-root",
                str(state.ROOT),
                "prune-stale",
                "--run-id",
                "expired-dispatch",
            ]
        )
        == 0
    )
    row = _rows(db)[0]
    assert row["status"] == "stale"
    assert row["returncode"] == custody.PROOF_QUEUE_STALE_EXIT_CODE
    out = capsys.readouterr().out
    assert "dispatch-handoff-expired" in out
    assert str(logs / "expired-dispatch.runner.log") in out


def test_proof_queue_named_lane_can_detach_runner(
    monkeypatch: pytest.MonkeyPatch, tmp_path: Path
) -> None:
    db = tmp_path / "proof_queue.sqlite3"
    logs = tmp_path / "runs"
    launched: dict[str, object] = {}

    def fake_launch(args: object, *, run_id: str, timeout: float) -> tuple[int, Path]:
        del args
        launched["run_id"] = run_id
        launched["timeout"] = timeout
        return 12345, logs / f"{run_id}.runner.log"

    monkeypatch.setattr(custody, "_launch_detached_runner", fake_launch)

    rc = cli.main(
        [
            "--db",
            str(db),
            "--logs-root",
            str(logs),
            "--repo-root",
            str(state.ROOT),
            "pact-witness-oracle",
            "--timeout",
            "42",
            "--detach",
            "--note",
            "detached queue launch smoke",
        ]
    )

    rows = _rows(db)
    assert rc == 0
    assert len(rows) == 1
    assert rows[0]["status"] == "dispatched"
    assert launched == {"run_id": rows[0]["run_id"], "timeout": 42.0}
    assert [note["body"] for note in _notes(db)][-1:] == ["detached queue launch smoke"]


def test_proof_queue_exec_detach_requires_submission_note(
    monkeypatch: pytest.MonkeyPatch,
    tmp_path: Path,
    capsys: pytest.CaptureFixture[str],
) -> None:
    db = tmp_path / "proof_queue.sqlite3"
    logs = tmp_path / "runs"

    def fail_launch(args: object, *, run_id: str, timeout: float) -> tuple[int, Path]:
        del args, run_id, timeout
        raise AssertionError("note-less detached row must not launch")

    monkeypatch.setattr(custody, "_launch_detached_runner", fail_launch)

    rc = cli.main(
        [
            "--db",
            str(db),
            "--logs-root",
            str(logs),
            "--repo-root",
            str(state.ROOT),
            "exec",
            "--id",
            "missing-note-detach",
            "--reason",
            "prove detached proof notes fail closed",
            "--resource-family",
            "python",
            "--contention-key",
            "python:missing-note-detach",
            "--scope",
            "tools/proof_queue.py",
            "--detach",
            "--",
            sys.executable,
            "-c",
            "print('must not run')",
        ]
    )

    assert rc == 2
    assert not db.exists()
    assert "queued proof submissions require at least one append-only note" in (
        capsys.readouterr().err
    )


def test_proof_queue_named_lane_can_queue_without_runner(
    monkeypatch: pytest.MonkeyPatch,
    tmp_path: Path,
    capsys: pytest.CaptureFixture[str],
) -> None:
    db = tmp_path / "proof_queue.sqlite3"
    logs = tmp_path / "runs"

    def fail_launch(args: object, *, run_id: str, timeout: float) -> tuple[int, Path]:
        del args, run_id, timeout
        raise AssertionError("--queue-only must not launch a detached runner")

    monkeypatch.setattr(custody, "_launch_detached_runner", fail_launch)

    rc = cli.main(
        [
            "--db",
            str(db),
            "--logs-root",
            str(logs),
            "--repo-root",
            str(state.ROOT),
            "r6-target-version-parity",
            "--python-version",
            "3.12",
            "--queue-only",
            "--note",
            "queue-only R6 parking smoke",
        ]
    )

    rows = _rows(db)
    stdout = capsys.readouterr().out.strip().splitlines()
    assert rc == 0
    assert stdout == [f"queued {rows[0]['run_id']}"]
    assert len(rows) == 1
    assert rows[0]["status"] == "queued"
    assert rows[0]["logical_id"] == "r6-target-version-parity-py312"
    assert rows[0]["contention_key"] == "python:r6-target-version-py312"
    log_text = Path(rows[0]["log_path"]).read_text(encoding="utf-8")
    assert "status=queued" in log_text
    assert "logical_id=r6-target-version-parity-py312" in log_text
    assert "No proof command has launched for this queued row." in log_text
    command = json.loads(rows[0]["command_json"])
    assert command[command.index("--python-version") + 1] == "3.12"
    assert [note["body"] for note in _notes(db)][-1:] == ["queue-only R6 parking smoke"]


def test_proof_queue_named_lane_rejects_queue_only_with_detach(
    tmp_path: Path,
) -> None:
    db = tmp_path / "proof_queue.sqlite3"
    logs = tmp_path / "runs"

    with pytest.raises(SystemExit) as exc:
        cli.main(
            [
                "--db",
                str(db),
                "--logs-root",
                str(logs),
                "--repo-root",
                str(state.ROOT),
                "r6-target-version-parity",
                "--queue-only",
                "--detach",
            ]
        )

    assert exc.value.code == 2
    assert not db.exists()


def test_proof_queue_queued_missing_log_is_not_running_evidence(
    tmp_path: Path, capsys: pytest.CaptureFixture[str]
) -> None:
    db = tmp_path / "proof_queue.sqlite3"
    logs = tmp_path / "runs"
    assert (
        cli.main(
            [
                "--db",
                str(db),
                "--logs-root",
                str(logs),
                "--repo-root",
                str(state.ROOT),
                "r6-target-version-parity",
                "--queue-only",
            ]
        )
        == 0
    )
    row = _rows(db)[0]
    Path(row["log_path"]).unlink()

    assert diagnostics_module._active_log_status(row) == [
        f"  log={Path(row['log_path'])} (queued; proof command not launched yet)"
    ]
    capsys.readouterr()
    assert (
        cli.main(
            [
                "--db",
                str(db),
                "--logs-root",
                str(logs),
                "--repo-root",
                str(state.ROOT),
                "audit",
                "--json",
                "--no-notebook-check",
            ]
        )
        == 0
    )
    payload = json.loads(capsys.readouterr().out)
    assert all(
        issue["signal_id"] != "audit-active-log-missing" for issue in payload["issues"]
    )


def test_proof_queue_windows_launchers_hide_console(
    monkeypatch: pytest.MonkeyPatch, tmp_path: Path
) -> None:
    captured: list[dict[str, object]] = []

    class FakePopen:
        pid = 12345

        def __init__(self, _command: list[str], **kwargs: object) -> None:
            captured.append(kwargs)

    monkeypatch.setattr(custody, "_queue_process_spawn_is_windows", lambda: True)
    monkeypatch.setattr(
        custody.subprocess,
        "CREATE_NEW_PROCESS_GROUP",
        0x00000200,
        raising=False,
    )
    monkeypatch.setattr(
        custody.subprocess,
        "CREATE_NO_WINDOW",
        0x08000000,
        raising=False,
    )
    monkeypatch.setattr(custody, "Popen", FakePopen)

    args = SimpleNamespace(
        db=tmp_path / "proof_queue.sqlite3",
        logs_root=tmp_path / "runs",
        notebooks_root=None,
        repo_root=tmp_path,
    )

    custody._launch_detached_runner(args, run_id="hidden-runner", timeout=1.0)

    assert captured[0]["creationflags"] == 0x08000200
    assert custody._queued_command_process_kwargs() == {"creationflags": 0x08000200}


def test_proof_queue_posix_detached_runner_uses_new_session(
    monkeypatch: pytest.MonkeyPatch, tmp_path: Path
) -> None:
    captured: list[dict[str, object]] = []

    class FakePopen:
        pid = 12345

        def __init__(self, _command: list[str], **kwargs: object) -> None:
            captured.append(kwargs)

    monkeypatch.setattr(custody, "_queue_process_spawn_is_windows", lambda: False)
    monkeypatch.setattr(custody, "Popen", FakePopen)

    args = SimpleNamespace(
        db=tmp_path / "proof_queue.sqlite3",
        logs_root=tmp_path / "runs",
        notebooks_root=None,
        repo_root=tmp_path,
    )

    custody._launch_detached_runner(args, run_id="posix-runner", timeout=1.0)

    assert captured[0]["start_new_session"] is True
    assert "creationflags" not in captured[0]
    assert custody._queued_command_process_kwargs() == {}


def test_proof_queue_rejects_uv_run_without_active_project_python(
    tmp_path: Path,
) -> None:
    db = tmp_path / "proof_queue.sqlite3"
    logs = tmp_path / "runs"

    rc = cli.main(
        [
            "--db",
            str(db),
            "--logs-root",
            str(logs),
            "--repo-root",
            str(state.ROOT),
            "exec",
            "--id",
            "bad-uv",
            "--reason",
            "reject throwaway uv env",
            "--resource-family",
            "python",
            "--contention-key",
            "python:bad-uv",
            "--",
            "uv",
            "run",
            "python",
            "-c",
            "print('should-not-run')",
        ]
    )

    rows = _rows(db)
    assert rc == 2
    assert len(rows) == 1
    assert rows[0]["status"] == "failed"
    assert rows[0]["returncode"] == 2
    log_text = Path(rows[0]["log_path"]).read_text(encoding="utf-8")
    assert "refuses `uv run`" in log_text
    assert "should-not-run" in log_text


def test_proof_queue_rejects_raw_cargo_exec(
    tmp_path: Path, capsys: pytest.CaptureFixture[str]
) -> None:
    db = tmp_path / "proof_queue.sqlite3"
    logs = tmp_path / "runs"

    rc = cli.main(
        [
            "--db",
            str(db),
            "--logs-root",
            str(logs),
            "--repo-root",
            str(state.ROOT),
            "exec",
            "--id",
            "raw-cargo",
            "--reason",
            "reject ad hoc cargo proof",
            "--resource-family",
            "rust",
            "--contention-key",
            "cargo:molt-runtime",
            "--",
            "cargo",
            "test",
            "-p",
            "molt-runtime",
            "--lib",
        ]
    )

    rows = _rows(db)
    assert rc == 2
    assert len(rows) == 1
    assert rows[0]["status"] == "failed"
    assert rows[0]["returncode"] == 2
    log_text = Path(rows[0]["log_path"]).read_text(encoding="utf-8")
    assert "refuses raw `cargo` commands" in log_text
    assert "proof_queue.py cargo" in log_text

    assert (
        cli.main(
            [
                "--db",
                str(db),
                "--logs-root",
                str(logs),
                "--repo-root",
                str(state.ROOT),
                "audit",
                "--no-notebook-check",
            ]
        )
        == 0
    )
    output = capsys.readouterr().out
    assert "audit-queue-policy-rejection" in output


def test_proof_queue_cargo_lane_records_guarded_uv_envelope(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    db = tmp_path / "proof_queue.sqlite3"
    logs = tmp_path / "runs"
    launched: dict[str, object] = {}

    def fake_launch(args: object, *, run_id: str, timeout: float) -> tuple[int, Path]:
        del args
        launched["run_id"] = run_id
        launched["timeout"] = timeout
        return 4242, tmp_path / "runner.log"

    monkeypatch.setattr(custody, "_launch_detached_runner", fake_launch)

    rc = cli.main(
        [
            "--db",
            str(db),
            "--logs-root",
            str(logs),
            "--repo-root",
            str(state.ROOT),
            "cargo",
            "--id",
            "runtime-focused-proof",
            "--reason",
            "prove runtime cargo lane",
            "--scope",
            "runtime/molt-runtime/src/cpython_abi_hooks.rs",
            "--note",
            "canonical cargo proof lane smoke",
            "--timeout",
            "42",
            "--detach",
            "--",
            "test",
            "-p",
            "molt-runtime",
            "--lib",
        ]
    )

    rows = _rows(db)
    assert rc == 0
    assert len(rows) == 1
    assert rows[0]["status"] == "dispatched"
    assert rows[0]["resource_family"] == "rust"
    assert rows[0]["contention_key"] == "cargo:molt-runtime"
    assert launched == {"run_id": rows[0]["run_id"], "timeout": 42.0}
    command = json.loads(rows[0]["command_json"])
    assert command[:9] == [
        "uv",
        "run",
        "--active",
        "--project",
        ".",
        "--python",
        "3.12",
        "--no-sync",
        "python",
    ]
    assert command[9:15] == [
        "tools/guarded_exec.py",
        "--prefix",
        "MOLT_TEST_SUITE",
        "--",
        "cargo",
        "test",
    ]
    assert command[15:17] == ["-p", "molt-runtime"]
    assert command[-1] == "--lib"
    assert [note["body"] for note in _notes(db)] == ["canonical cargo proof lane smoke"]
    notebook = tmp_path / "notebooks" / f"{rows[0]['run_id']}.py"
    assert notebook.exists()
    assert '"proof_receipt"' not in notebook.read_text(encoding="utf-8")

    requested_toolchains: list[tuple[str, ...]] = []

    def fake_fingerprints(_plan: object, names: tuple[str, ...]) -> dict[str, dict]:
        requested_toolchains.append(names)
        return {name: {"identity_sha256": name} for name in names}

    monkeypatch.setattr(
        evidence_module.proof_plan, "toolchain_fingerprints", fake_fingerprints
    )
    conn = state._connect(db)
    state._update_run(conn, rows[0]["run_id"], status="passed", returncode=0)
    conn.row_factory = sqlite3.Row
    terminal_row = conn.execute(
        "SELECT * FROM proof_runs WHERE run_id = ?", (rows[0]["run_id"],)
    ).fetchone()
    assert terminal_row is not None
    terminal_payload = evidence_module._row_to_payload(terminal_row)
    conn.close()

    assert requested_toolchains == [("python", "cargo", "rustc")]
    assert terminal_payload["proof_receipt"]["status"] == "success"


def test_proof_queue_cargo_rejects_pre_delimiter_residue(tmp_path: Path) -> None:
    db = tmp_path / "proof_queue.sqlite3"
    logs = tmp_path / "runs"

    with pytest.raises(SystemExit, match="stray positional argument"):
        cli.main(
            [
                "--db",
                str(db),
                "--logs-root",
                str(logs),
                "--repo-root",
                str(state.ROOT),
                "cargo",
                "--id",
                "bad-cargo-shell-quote",
                "--reason",
                '"Prove',
                "runtime",
                "cargo",
                "--scope",
                "runtime/molt-runtime/src/cpython_abi_hooks.rs",
                "--note",
                "this metadata would be silently swallowed before the fix",
                "--",
                "test",
                "-p",
                "molt-runtime",
                "--lib",
            ]
        )

    assert not db.exists()


def test_proof_queue_cargo_requires_command_delimiter(tmp_path: Path) -> None:
    db = tmp_path / "proof_queue.sqlite3"
    logs = tmp_path / "runs"

    with pytest.raises(SystemExit, match="requires `--` before the proof command"):
        cli.main(
            [
                "--db",
                str(db),
                "--logs-root",
                str(logs),
                "--repo-root",
                str(state.ROOT),
                "cargo",
                "--id",
                "missing-cargo-delimiter",
                "--reason",
                "prove cargo delimiter guard",
                "test",
                "-p",
                "molt-runtime",
                "--lib",
            ]
        )

    assert not db.exists()


def test_proof_queue_cargo_lane_rejects_cold_single_lib_test(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    db = tmp_path / "proof_queue.sqlite3"
    logs = tmp_path / "runs"
    launched: list[str] = []

    def fake_launch(args: object, *, run_id: str, timeout: float) -> tuple[int, Path]:
        del args, timeout
        launched.append(run_id)
        return 4242, tmp_path / "runner.log"

    monkeypatch.setattr(custody, "_launch_detached_runner", fake_launch)

    rc = cli.main(
        [
            "--db",
            str(db),
            "--logs-root",
            str(logs),
            "--repo-root",
            str(state.ROOT),
            "cargo",
            "--id",
            "cold-single-lib-test",
            "--reason",
            "reject cold single-test cargo proof",
            "--scope",
            "runtime/molt-runtime/src/cpython_abi_hooks.rs",
            "--note",
            "synthetic violation: exact lib test without warm-target override",
            "--timeout",
            "42",
            "--detach",
            "--",
            "test",
            "-p",
            "molt-runtime",
            "pyinit_module_to_bits_reports_static_link_py_mod_exec_pending_error",
            "--lib",
        ]
    )

    rows = _rows(db)
    assert rc == 2
    assert len(rows) == 1
    assert rows[0]["status"] == "failed"
    assert rows[0]["returncode"] == 2
    assert launched == []
    log_text = Path(rows[0]["log_path"]).read_text(encoding="utf-8")
    assert "refuses cold-prone single-test Cargo proofs" in log_text
    assert "Batch the relevant crate shard" in log_text
    assert "--allow-warm-single-test" in log_text


def test_proof_queue_cargo_lane_allows_explicit_warm_single_test(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    db = tmp_path / "proof_queue.sqlite3"
    logs = tmp_path / "runs"
    launched: dict[str, object] = {}

    def fake_launch(args: object, *, run_id: str, timeout: float) -> tuple[int, Path]:
        del args
        launched["run_id"] = run_id
        launched["timeout"] = timeout
        return 4242, tmp_path / "runner.log"

    monkeypatch.setattr(custody, "_launch_detached_runner", fake_launch)

    rc = cli.main(
        [
            "--db",
            str(db),
            "--logs-root",
            str(logs),
            "--repo-root",
            str(state.ROOT),
            "cargo",
            "--id",
            "warm-single-lib-test",
            "--reason",
            "allow explicit warm single-test cargo proof",
            "--scope",
            "runtime/molt-runtime/src/cpython_abi_hooks.rs",
            "--note",
            "warmup: cargo check -p molt-runtime already completed in this target dir",
            "--timeout",
            "42",
            "--detach",
            "--allow-warm-single-test",
            "--",
            "test",
            "-p",
            "molt-runtime",
            "pyinit_module_to_bits_reports_static_link_py_mod_exec_pending_error",
            "--lib",
        ]
    )

    rows = _rows(db)
    assert rc == 0
    assert len(rows) == 1
    assert rows[0]["status"] == "dispatched"
    assert launched == {"run_id": rows[0]["run_id"], "timeout": 42.0}
    command = json.loads(rows[0]["command_json"])
    assert (
        "pyinit_module_to_bits_reports_static_link_py_mod_exec_pending_error" in command
    )
    notes = [note["body"] for note in _notes(db)]
    assert notes[0].startswith("policy: --allow-warm-single-test used")
    assert notes[1] == (
        "warmup: cargo check -p molt-runtime already completed in this target dir"
    )


def test_proof_queue_submit_run_executes_queued_row_in_place(tmp_path: Path) -> None:
    db = tmp_path / "proof_queue.sqlite3"
    logs = tmp_path / "runs"
    dsl = tmp_path / "proof.toml"
    dsl.write_text(
        "\n".join(
            [
                "[[proof]]",
                'id = "queued-proof"',
                'reason = "prove queued row"',
                'resource_family = "python"',
                'contention_key = "python:queued"',
                'env = { PROOF_QUEUE_TEST = "queued-ok" }',
                f'command = [{sys.executable!r}, "-c", "import os; print(os.environ[\'PROOF_QUEUE_TEST\'])"]',
            ]
        ),
        encoding="utf-8",
    )

    assert (
        cli.main(
            [
                "--db",
                str(db),
                "--logs-root",
                str(logs),
                "--repo-root",
                str(state.ROOT),
                "submit",
                str(dsl),
            ]
        )
        == 0
    )
    rows = _rows(db)
    queued_log_text = Path(rows[0]["log_path"]).read_text(encoding="utf-8")
    assert "status=queued" in queued_log_text
    assert "logical_id=queued-proof" in queued_log_text
    assert "env_overrides=" in queued_log_text
    assert "No proof command has launched for this queued row." in queued_log_text
    assert (
        cli.main(
            [
                "--db",
                str(db),
                "--logs-root",
                str(logs),
                "--repo-root",
                str(state.ROOT),
                "run",
                "--timeout",
                "30",
            ]
        )
        == 0
    )

    rows = _rows(db)
    assert len(rows) == 1
    assert rows[0]["status"] == "passed"
    assert "queued-ok" in Path(rows[0]["log_path"]).read_text(encoding="utf-8")


def test_proof_queue_submit_records_initial_notes_and_marimo_projection(
    tmp_path: Path,
) -> None:
    db = tmp_path / "proof_queue.sqlite3"
    logs = tmp_path / "runs"
    notebooks = tmp_path / "notebooks"
    dsl = tmp_path / "proof.toml"
    dsl.write_text(
        "\n".join(
            [
                "[[proof]]",
                'id = "queued-notebook-proof"',
                'reason = "capture proof intent"',
                'resource_family = "python"',
                'contention_key = "python:queued-notebook"',
                'note = "changed typed-buffer descriptor authority"',
                'notes = ["testing queue-owned lab notebook projection"]',
                f'command = [{sys.executable!r}, "-c", "print(\'queued\')"]',
            ]
        ),
        encoding="utf-8",
    )

    assert (
        cli.main(
            [
                "--db",
                str(db),
                "--logs-root",
                str(logs),
                "--notebooks-root",
                str(notebooks),
                "--repo-root",
                str(state.ROOT),
                "submit",
                str(dsl),
            ]
        )
        == 0
    )

    rows = _rows(db)
    notes = _notes(db)
    assert len(rows) == 1
    assert [note["kind"] for note in notes] == ["submission", "submission"]
    assert [note["body"] for note in notes] == [
        "changed typed-buffer descriptor authority",
        "testing queue-owned lab notebook projection",
    ]
    notebook = notebooks / f"{rows[0]['run_id']}.py"
    notebook_text = notebook.read_text(encoding="utf-8")
    assert "import marimo" in notebook_text
    assert "changed typed-buffer descriptor authority" in notebook_text
    assert '"git": {' in notebook_text


def test_proof_queue_submit_records_dag_edges_and_runs_ready_order(
    tmp_path: Path,
) -> None:
    db = tmp_path / "proof_queue.sqlite3"
    logs = tmp_path / "runs"
    notebooks = tmp_path / "notebooks"
    dsl = tmp_path / "proof.toml"
    dsl.write_text(
        "\n".join(
            [
                "[[proof]]",
                'id = "child-proof"',
                'reason = "prove child waits"',
                'resource_family = "python"',
                'contention_key = "python:parent-child"',
                'depends_on = ["parent-proof"]',
                # depends_on is the only scheduling edge kind; lineage kinds
                # (derives_from etc.) record provenance without gating.
                'edge_kind = "depends_on"',
                'edge_note = "Child narrows the parent proof result."',
                f'command = [{sys.executable!r}, "-c", "print(\'child\')"]',
                "",
                "[[proof]]",
                'id = "parent-proof"',
                'reason = "prove parent first"',
                'resource_family = "python"',
                'contention_key = "python:parent-child"',
                f'command = [{sys.executable!r}, "-c", "print(\'parent\')"]',
            ]
        ),
        encoding="utf-8",
    )

    assert (
        cli.main(
            [
                "--db",
                str(db),
                "--logs-root",
                str(logs),
                "--notebooks-root",
                str(notebooks),
                "--repo-root",
                str(state.ROOT),
                "submit",
                str(dsl),
            ]
        )
        == 0
    )

    rows = _rows(db)
    child = next(row for row in rows if row["logical_id"] == "child-proof")
    parent = next(row for row in rows if row["logical_id"] == "parent-proof")
    edges = _edges(db)
    assert len(edges) == 1
    assert edges[0]["parent_run_id"] == parent["run_id"]
    assert edges[0]["child_run_id"] == child["run_id"]
    assert edges[0]["kind"] == "depends_on"
    assert edges[0]["note"] == "Child narrows the parent proof result."

    assert (
        cli.main(
            [
                "--db",
                str(db),
                "--logs-root",
                str(logs),
                "--notebooks-root",
                str(notebooks),
                "--repo-root",
                str(state.ROOT),
                "run",
                "--timeout",
                "30",
                "--limit",
                "1",
            ]
        )
        == 0
    )
    rows = _rows(db)
    child = next(row for row in rows if row["logical_id"] == "child-proof")
    parent = next(row for row in rows if row["logical_id"] == "parent-proof")
    assert parent["status"] == "passed"
    assert child["status"] == "queued"

    assert (
        cli.main(
            [
                "--db",
                str(db),
                "--logs-root",
                str(logs),
                "--notebooks-root",
                str(notebooks),
                "--repo-root",
                str(state.ROOT),
                "run",
                "--timeout",
                "30",
            ]
        )
        == 0
    )
    rows = _rows(db)
    child = next(row for row in rows if row["logical_id"] == "child-proof")
    assert child["status"] == "passed"
    notebook_text = (notebooks / f"{child['run_id']}.py").read_text(encoding="utf-8")
    assert '"parent_kind_counts": {' in notebook_text
    assert '"depends_on": 1' in notebook_text


def test_proof_queue_blocked_dependency_writes_evidence_without_missing_log_debt(
    tmp_path: Path, capsys: pytest.CaptureFixture[str]
) -> None:
    db = tmp_path / "proof_queue.sqlite3"
    logs = tmp_path / "runs"
    notebooks = tmp_path / "notebooks"
    conn = state._connect(db)
    for run_id, status in (("failed-parent", "failed"), ("blocked-child", "queued")):
        scheduling._insert_run(
            conn,
            run_id=run_id,
            logical_id=run_id,
            reason="prove blocked dependency evidence",
            command=[sys.executable, "-c", "print('blocked')"],
            cwd=state.ROOT,
            resource_family="python",
            contention_key=f"python:{run_id}",
            scopes=["tools/proof_queue.py"],
            git_snapshot={
                "available": True,
                "head": "abc123",
                "dirty": False,
                "status": [],
            },
            log_path=logs / f"{run_id}.log",
            summary_json=logs / f"{run_id}.memory_guard.json",
        )
        state._update_run(conn, run_id, status=status)
    state._insert_note(
        conn,
        run_id="blocked-child",
        body="test: blocked dependency must leave evidence",
        kind="submission",
        author="codex",
    )
    state._insert_edge(
        conn,
        parent_run_id="failed-parent",
        child_run_id="blocked-child",
        kind="depends_on",
        note="child waits on failed parent",
    )

    assert (
        cli.main(
            [
                "--db",
                str(db),
                "--logs-root",
                str(logs),
                "--notebooks-root",
                str(notebooks),
                "--repo-root",
                str(state.ROOT),
                "run",
            ]
        )
        == 0
    )
    child_log = logs / "blocked-child.log"
    assert "proof queue blocked by dependency" in child_log.read_text(encoding="utf-8")
    assert (notebooks / "blocked-child.py").exists()
    capsys.readouterr()

    assert (
        cli.main(
            [
                "--db",
                str(db),
                "--logs-root",
                str(logs),
                "--repo-root",
                str(state.ROOT),
                "evidence",
                "--run-id",
                "blocked-child",
            ]
        )
        == 0
    )
    evidence = json.loads(capsys.readouterr().out)
    assert evidence[0]["status"] == "blocked"
    assert [item["signal_id"] for item in evidence[0]["diagnostics"]] == [
        "proof-dependency-blocked"
    ]

    assert (
        cli.main(
            [
                "--db",
                str(db),
                "--logs-root",
                str(logs),
                "--repo-root",
                str(state.ROOT),
                "audit",
                "--no-notebook-check",
            ]
        )
        == 1
    )
    output = capsys.readouterr().out
    assert "proof-log-missing" in output
    assert "run=failed-parent" in output
    assert "run=blocked-child" not in output


def test_proof_queue_status_reconciles_blocked_queued_dependency(
    tmp_path: Path, capsys: pytest.CaptureFixture[str]
) -> None:
    db = tmp_path / "proof_queue.sqlite3"
    logs = tmp_path / "runs"
    notebooks = tmp_path / "notebooks"
    _insert_blocked_dependency_fixture(db, logs)

    assert (
        cli.main(
            [
                "--db",
                str(db),
                "--logs-root",
                str(logs),
                "--notebooks-root",
                str(notebooks),
                "--repo-root",
                str(state.ROOT),
                "status",
                "--recent",
                "5",
            ]
        )
        == 0
    )

    rows = _rows(db)
    child = next(row for row in rows if row["run_id"] == "blocked-child")
    out = capsys.readouterr().out
    assert child["status"] == "blocked"
    assert "active:\n- none" in out
    assert "blocked-child" in out
    assert "proof-dependency-blocked" in out
    assert "proof queue blocked by dependency" in (
        logs / "blocked-child.log"
    ).read_text(encoding="utf-8")
    assert (notebooks / "blocked-child.py").exists()


def test_proof_queue_submission_reconciles_blocked_dependency_before_contention(
    tmp_path: Path,
) -> None:
    db = tmp_path / "proof_queue.sqlite3"
    logs = tmp_path / "runs"
    notebooks = tmp_path / "notebooks"
    contention_key = "python:blocked-slot"
    _insert_blocked_dependency_fixture(db, logs, contention_key=contention_key)
    args = SimpleNamespace(
        db=db,
        logs_root=logs,
        notebooks_root=notebooks,
        repo_root=state.ROOT,
    )

    rc, run_id = runner._queue_one(
        args,
        logical_id="new-proof",
        reason="prove freed contention after dependency reconciliation",
        command=[sys.executable, "-c", "print('new')"],
        resource_family="python",
        contention_key=contention_key,
        scopes=["tools/proof_queue.py"],
        env_overrides={},
        initial_notes=["test: blocked dependency must not hold contention"],
    )

    assert rc == 0
    assert run_id is not None
    rows = _rows(db)
    child = next(row for row in rows if row["run_id"] == "blocked-child")
    new_row = next(row for row in rows if row["run_id"] == run_id)
    assert child["status"] == "blocked"
    assert new_row["status"] == "queued"
    assert new_row["contention_key"] == contention_key


def test_proof_queue_lineage_edges_do_not_gate_execution(
    tmp_path: Path,
) -> None:
    db = tmp_path / "proof_queue.sqlite3"
    logs = tmp_path / "runs"
    conn = state._connect(db)
    for run_id, status in (("failed-parent", "failed"), ("rerun-child", "queued")):
        scheduling._insert_run(
            conn,
            run_id=run_id,
            logical_id=run_id,
            reason="prove lineage edges never gate",
            command=[sys.executable, "-c", "print('rerun')"],
            cwd=state.ROOT,
            resource_family="python",
            contention_key=f"python:{run_id}",
            scopes=["tools/proof_queue.py"],
            git_snapshot={
                "available": True,
                "head": "abc123",
                "dirty": False,
                "status": [],
            },
            log_path=logs / f"{run_id}.log",
            summary_json=logs / f"{run_id}.memory_guard.json",
        )
        state._update_run(conn, run_id, status=status)
    # A rerun's parent is failed or stale by definition: lineage kinds
    # preserve provenance and must never gate scheduling (PROOF_QUEUE.md:
    # "depends_on is the scheduling edge; the others preserve lineage").
    for kind in ("reruns", "supersedes", "compares", "derives_from"):
        state._insert_edge(
            conn,
            parent_run_id="failed-parent",
            child_run_id="rerun-child",
            kind=kind,
            note=f"lineage edge {kind}",
        )

    dependency_state, blockers = scheduling._dependency_state(conn, "rerun-child")

    assert dependency_state == "ready"
    assert blockers == []

    state._insert_edge(
        conn,
        parent_run_id="failed-parent",
        child_run_id="rerun-child",
        kind="depends_on",
        note="scheduling edge still gates",
    )
    dependency_state, blockers = scheduling._dependency_state(conn, "rerun-child")
    assert dependency_state == "blocked"
    assert [row["kind"] for row in blockers] == ["depends_on"]


def test_proof_queue_lineage_edges_accept_external_parent_ids(
    tmp_path: Path,
) -> None:
    db = tmp_path / "proof_queue.sqlite3"
    logs = tmp_path / "runs"
    conn = state._connect(db)
    scheduling._insert_run(
        conn,
        run_id="local-child",
        logical_id="local-child",
        reason="prove external lineage parent contract",
        command=[sys.executable, "-c", "print('child')"],
        cwd=state.ROOT,
        resource_family="python",
        contention_key="python:local-child",
        scopes=["tools/proof_queue.py"],
        git_snapshot={
            "available": True,
            "head": "abc123",
            "dirty": False,
            "status": [],
        },
        log_path=logs / "local-child.log",
        summary_json=logs / "local-child.memory_guard.json",
    )
    fk_columns = {
        row[3] for row in conn.execute("PRAGMA foreign_key_list(proof_run_edges)")
    }
    assert "parent_run_id" not in fk_columns
    assert "child_run_id" in fk_columns

    state._insert_edge(
        conn,
        parent_run_id="external-worktree-parent",
        child_run_id="local-child",
        kind="supersedes",
        note="external queue DB lineage",
    )
    edges = state._edges_for_run_ids(conn, ["local-child"])
    parents = edges["local-child"]["parents"]
    assert len(parents) == 1
    assert parents[0]["parent_run_id"] == "external-worktree-parent"
    assert parents[0]["parent_status"] is None
    assert parents[0]["kind"] == "supersedes"
    assert scheduling._dependency_state(conn, "local-child") == ("ready", [])

    with pytest.raises(SystemExit, match="unknown parent proof run"):
        state._insert_edge(
            conn,
            parent_run_id="external-worktree-parent",
            child_run_id="local-child",
            kind="depends_on",
        )


def test_proof_queue_migrates_edge_parent_fk_to_external_lineage(
    tmp_path: Path,
) -> None:
    db = tmp_path / "legacy_proof_queue.sqlite3"
    legacy = sqlite3.connect(db)
    legacy.execute("PRAGMA foreign_keys=ON")
    legacy.execute(
        """
        CREATE TABLE proof_runs (
            run_id TEXT PRIMARY KEY,
            logical_id TEXT NOT NULL,
            reason TEXT NOT NULL,
            status TEXT NOT NULL,
            returncode INTEGER,
            command_json TEXT NOT NULL,
            cwd TEXT NOT NULL,
            resource_family TEXT NOT NULL,
            contention_key TEXT NOT NULL,
            scopes_json TEXT NOT NULL,
            env_json TEXT NOT NULL DEFAULT '{}',
            git_json TEXT NOT NULL DEFAULT '{}',
            log_path TEXT NOT NULL,
            summary_json TEXT NOT NULL,
            guard_pid INTEGER,
            guard_identity TEXT,
            started_at TEXT,
            finished_at TEXT,
            elapsed_s REAL
        )
        """
    )
    legacy.execute(
        """
        CREATE TABLE proof_run_edges (
            edge_id INTEGER PRIMARY KEY AUTOINCREMENT,
            parent_run_id TEXT NOT NULL,
            child_run_id TEXT NOT NULL,
            created_at TEXT NOT NULL,
            author TEXT NOT NULL,
            kind TEXT NOT NULL,
            note TEXT NOT NULL DEFAULT '',
            FOREIGN KEY(parent_run_id) REFERENCES proof_runs(run_id),
            FOREIGN KEY(child_run_id) REFERENCES proof_runs(run_id),
            UNIQUE(parent_run_id, child_run_id, kind)
        )
        """
    )
    run_row = (
        "parent-run",
        "parent-run",
        "legacy parent",
        "failed",
        1,
        "[]",
        str(state.ROOT),
        "python",
        "python:parent",
        "[]",
        "{}",
        "{}",
        str(tmp_path / "parent.log"),
        str(tmp_path / "parent.memory_guard.json"),
    )
    child_row = (
        "child-run",
        "child-run",
        "legacy child",
        "queued",
        None,
        "[]",
        str(state.ROOT),
        "python",
        "python:child",
        "[]",
        "{}",
        "{}",
        str(tmp_path / "child.log"),
        str(tmp_path / "child.memory_guard.json"),
    )
    legacy.executemany(
        """
        INSERT INTO proof_runs (
            run_id, logical_id, reason, status, returncode, command_json, cwd,
            resource_family, contention_key, scopes_json, env_json, git_json,
            log_path, summary_json
        )
        VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
        """,
        [run_row, child_row],
    )
    legacy.execute(
        """
        INSERT INTO proof_run_edges (
            parent_run_id, child_run_id, created_at, author, kind, note
        )
        VALUES ('parent-run', 'child-run', '2026-07-05T00:00:00Z',
                'codex', 'supersedes', 'legacy edge')
        """
    )
    assert {
        row[3] for row in legacy.execute("PRAGMA foreign_key_list(proof_run_edges)")
    } == {"parent_run_id", "child_run_id"}
    legacy.commit()
    legacy.close()

    conn = state._connect(db)

    fk_columns = {
        row[3] for row in conn.execute("PRAGMA foreign_key_list(proof_run_edges)")
    }
    assert "parent_run_id" not in fk_columns
    assert "child_run_id" in fk_columns
    edges = _edges(db)
    assert len(edges) == 1
    assert edges[0]["parent_run_id"] == "parent-run"
    assert edges[0]["child_run_id"] == "child-run"
    assert edges[0]["note"] == "legacy edge"


def test_proof_queue_submission_allows_external_lineage_parent(
    tmp_path: Path,
) -> None:
    db = tmp_path / "proof_queue.sqlite3"
    logs = tmp_path / "runs"
    args = SimpleNamespace(
        db=db,
        logs_root=logs,
        notebooks_root=tmp_path / "notebooks",
        repo_root=state.ROOT,
    )

    rc, run_id = runner._queue_one(
        args,
        logical_id="external-lineage-child",
        reason="prove queued external lineage parent",
        command=[sys.executable, "-c", "print('external lineage')"],
        resource_family="python",
        contention_key="python:external-lineage-child",
        scopes=["tools/proof_queue.py"],
        env_overrides={},
        initial_notes=["external lineage parent can live in another queue DB"],
        depends_on=["other-worktree-run"],
        edge_kind="supersedes",
        edge_note="record cross-worktree lineage without scheduling",
    )

    assert rc == 0
    assert run_id is not None
    conn = state._connect(db)
    edges = state._edges_for_run_ids(conn, [run_id])
    assert edges[run_id]["parents"][0]["parent_run_id"] == "other-worktree-run"
    assert edges[run_id]["parents"][0]["parent_status"] is None
    row = next(row for row in _rows(db) if row["run_id"] == run_id)
    assert row["status"] == "queued"

    with pytest.raises(SystemExit, match="unknown parent proof run"):
        runner._queue_one(
            args,
            logical_id="external-scheduling-child",
            reason="prove scheduling parents still fail closed",
            command=[sys.executable, "-c", "print('external scheduling')"],
            resource_family="python",
            contention_key="python:external-scheduling-child",
            scopes=["tools/proof_queue.py"],
            env_overrides={},
            initial_notes=["scheduling parent must exist in this queue DB"],
            depends_on=["other-worktree-run"],
            edge_kind="depends_on",
        )


def test_proof_queue_appends_notes_and_exports_evidence(
    tmp_path: Path, capsys: pytest.CaptureFixture[str]
) -> None:
    db = tmp_path / "proof_queue.sqlite3"
    notebooks = tmp_path / "notebooks"
    conn = state._connect(db)
    scheduling._insert_run(
        conn,
        run_id="noted-run",
        logical_id="noted",
        reason="prove append-only notes",
        command=[sys.executable, "-c", "print('noted')"],
        cwd=state.ROOT,
        resource_family="python",
        contention_key="python:noted",
        scopes=["tools/proof_queue.py"],
        git_snapshot={
            "available": True,
            "head": "abc123",
            "dirty": False,
            "status": [],
        },
        log_path=tmp_path / "noted.log",
        summary_json=tmp_path / "noted.memory_guard.json",
    )

    assert (
        cli.main(
            [
                "--db",
                str(db),
                "--logs-root",
                str(tmp_path / "runs"),
                "--notebooks-root",
                str(notebooks),
                "--repo-root",
                str(state.ROOT),
                "note",
                "noted-run",
                "--kind",
                "observation",
                "--author",
                "codex",
                "--note",
                "R18 is still running, so this note preserves observation context",
            ]
        )
        == 0
    )

    notes = _notes(db)
    assert len(notes) == 1
    assert notes[0]["kind"] == "observation"
    assert notes[0]["author"] == "codex"
    notebook_text = (notebooks / "noted-run.py").read_text(encoding="utf-8")
    assert "abc123" in notebook_text
    assert "R18 is still running" in notebook_text

    capsys.readouterr()
    assert (
        cli.main(
            [
                "--db",
                str(db),
                "--logs-root",
                str(tmp_path / "runs"),
                "--repo-root",
                str(state.ROOT),
                "evidence",
                "--run-id",
                "noted-run",
            ]
        )
        == 0
    )
    payload = capsys.readouterr().out
    evidence = json.loads(payload)
    assert '"notes": [' in payload
    assert '"head": "abc123"' in payload
    assert evidence[0]["note_kind_counts"] == {"observation": 1}
    assert "R18 is still running" in payload


def test_proof_queue_note_projection_failure_preserves_note(
    monkeypatch: pytest.MonkeyPatch, tmp_path: Path
) -> None:
    db = tmp_path / "proof_queue.sqlite3"
    log_path = tmp_path / "noted-warning.log"
    conn = state._connect(db)
    scheduling._insert_run(
        conn,
        run_id="noted-warning-run",
        logical_id="noted-warning",
        reason="prove note survives notebook projection failure",
        command=[sys.executable, "-c", "print('noted')"],
        cwd=state.ROOT,
        resource_family="python",
        contention_key="python:noted-warning",
        scopes=["tools/proof_queue.py"],
        git_snapshot={
            "available": True,
            "head": "abc123",
            "dirty": False,
            "status": [],
        },
        log_path=log_path,
        summary_json=tmp_path / "noted-warning.memory_guard.json",
    )

    def fail_notebook(*_args: object, **_kwargs: object) -> Path:
        raise RuntimeError("note notebook exploded")

    monkeypatch.setattr(evidence_module, "_write_marimo_notebook", fail_notebook)

    assert (
        cli.main(
            [
                "--db",
                str(db),
                "--logs-root",
                str(tmp_path / "runs"),
                "--repo-root",
                str(state.ROOT),
                "note",
                "noted-warning-run",
                "--kind",
                "observation",
                "--note",
                "manual note must survive projection failure",
            ]
        )
        == 0
    )

    notes = _notes(db)
    assert notes[0]["body"] == "manual note must survive projection failure"
    assert notes[0]["kind"] == "observation"
    assert notes[1]["kind"] == "finding"
    assert (
        "queue nonfatal infrastructure failure during note projection"
        in notes[1]["body"]
    )
    log_text = log_path.read_text(encoding="utf-8")
    assert (
        "proof queue nonfatal infrastructure failure during note projection" in log_text
    )
    assert "RuntimeError: note notebook exploded" in log_text


def test_proof_queue_diagnoses_runtime_wasm_missing_required_exports(
    tmp_path: Path, capsys: pytest.CaptureFixture[str]
) -> None:
    db = tmp_path / "proof_queue.sqlite3"
    log_path = tmp_path / "failed.log"
    conn = state._connect(db)
    scheduling._insert_run(
        conn,
        run_id="failed-run",
        logical_id="pact-witness-acceptance",
        reason="prove runtime export obligation diagnosis",
        command=[sys.executable, "-c", "print('fail')"],
        cwd=state.ROOT,
        resource_family="wasm",
        contention_key="wasm:pact-witness",
        scopes=["tools/proof_queue.py"],
        git_snapshot={
            "available": True,
            "head": "abc123",
            "dirty": False,
            "status": [],
        },
        log_path=log_path,
        summary_json=tmp_path / "failed.memory_guard.json",
    )
    log_path.write_text(
        "Runtime wasm build produced artifact missing required exports: "
        "PyArg_ParseTuple, PyArg_ParseTupleAndKeywords, PyArg_UnpackTuple, "
        "PyArg_VaParseTupleAndKeywords, PyErr_Format, PyErr_FormatV, "
        "PyObject_CallFunction\n"
        "Runtime wasm build failed\n",
        encoding="utf-8",
    )
    state._update_run(conn, "failed-run", status="failed", returncode=1)

    assert (
        cli.main(
            [
                "--db",
                str(db),
                "--logs-root",
                str(tmp_path / "runs"),
                "--repo-root",
                str(state.ROOT),
                "evidence",
                "failed-run",
            ]
        )
        == 0
    )
    evidence = json.loads(capsys.readouterr().out)
    diagnostics = evidence[0]["diagnostics"]
    assert diagnostics[0]["signal_id"] == "runtime-wasm-missing-required-exports"
    assert "PyErr_Format" in diagnostics[0]["summary"]
    assert "(+1 more)" in diagnostics[0]["summary"]
    assert "wasm_runtime_shared_export_link_args" in diagnostics[0]["next_action"]


def test_proof_queue_diagnoses_runtime_export_authority_unknown_name(
    tmp_path: Path, capsys: pytest.CaptureFixture[str]
) -> None:
    db = tmp_path / "proof_queue.sqlite3"
    log_path = tmp_path / "failed.log"
    conn = state._connect(db)
    scheduling._insert_run(
        conn,
        run_id="failed-run",
        logical_id="pact-witness-acceptance",
        reason="prove export authority unknown-name diagnosis",
        command=[sys.executable, "-c", "print('fail')"],
        cwd=state.ROOT,
        resource_family="wasm",
        contention_key="wasm:pact-witness",
        scopes=["tools/proof_queue.py"],
        git_snapshot={
            "available": True,
            "head": "abc123",
            "dirty": False,
            "status": [],
        },
        log_path=log_path,
        summary_json=tmp_path / "failed.memory_guard.json",
    )
    log_path.write_text(
        "ValueError: unknown WASM runtime import/export name: PyObject_Init\n",
        encoding="utf-8",
    )
    state._update_run(conn, "failed-run", status="failed", returncode=1)

    assert (
        cli.main(
            [
                "--db",
                str(db),
                "--logs-root",
                str(tmp_path / "runs"),
                "--repo-root",
                str(state.ROOT),
                "evidence",
                "failed-run",
            ]
        )
        == 0
    )
    evidence = json.loads(capsys.readouterr().out)
    diagnostics = evidence[0]["diagnostics"]
    assert diagnostics[0]["signal_id"] == "wasm-runtime-export-authority-unknown-name"
    assert "PyObject_Init" in diagnostics[0]["summary"]
    assert "generated WASM ABI link authority" in diagnostics[0]["next_action"]


def test_proof_queue_diagnoses_failed_static_module_exec(
    tmp_path: Path, capsys: pytest.CaptureFixture[str]
) -> None:
    db = tmp_path / "proof_queue.sqlite3"
    notebooks = tmp_path / "notebooks"
    log_path = tmp_path / "failed.log"
    conn = state._connect(db)
    scheduling._insert_run(
        conn,
        run_id="failed-run",
        logical_id="pact-witness-acceptance",
        reason="prove deterministic diagnosis",
        command=[sys.executable, "-c", "print('fail')"],
        cwd=state.ROOT,
        resource_family="wasm",
        contention_key="wasm:pact-witness",
        scopes=["tools/proof_queue.py"],
        git_snapshot={
            "available": True,
            "head": "abc123",
            "dirty": False,
            "status": [],
        },
        log_path=log_path,
        summary_json=tmp_path / "failed.memory_guard.json",
    )
    log_path.write_text(
        "Error: Unhandled Molt exception: ImportError: "
        "_nd_image: static-link PyModuleDef Py_mod_exec slot returned non-zero\n"
        f"diagnostic_json={tmp_path / 'static_extension_init_failure.json'}\n",
        encoding="utf-8",
    )
    state._update_run(conn, "failed-run", status="failed", returncode=1)

    assert (
        cli.main(
            [
                "--db",
                str(db),
                "--logs-root",
                str(tmp_path / "runs"),
                "--repo-root",
                str(state.ROOT),
                "evidence",
                "--run-id",
                "failed-run",
            ]
        )
        == 0
    )
    evidence = json.loads(capsys.readouterr().out)
    diagnostics = evidence[0]["diagnostics"]
    assert diagnostics[0]["signal_id"] == "static-pymodexec-nonzero"
    assert "_nd_image" in diagnostics[0]["summary"]
    assert diagnostics[0]["artifacts"] == [
        str(tmp_path / "static_extension_init_failure.json")
    ]

    assert (
        cli.main(
            [
                "--db",
                str(db),
                "--logs-root",
                str(tmp_path / "runs"),
                "--notebooks-root",
                str(notebooks),
                "--repo-root",
                str(state.ROOT),
                "diagnose",
                "failed-run",
                "--append-note",
                "--author",
                "codex",
            ]
        )
        == 0
    )
    diagnosis_text = capsys.readouterr().out
    assert "static-pymodexec-nonzero" in diagnosis_text
    assert "static_extension_init_failure.json" in diagnosis_text
    notes = _notes(db)
    assert notes[-1]["kind"] == "finding"
    assert "static-pymodexec-nonzero" in notes[-1]["body"]
    assert "static_extension_init_failure.json" in notes[-1]["body"]
    assert (notebooks / "failed-run.py").exists()


def test_proof_queue_diagnoses_numpy_wrapped_static_module_exec(
    tmp_path: Path, capsys: pytest.CaptureFixture[str]
) -> None:
    db = tmp_path / "proof_queue.sqlite3"
    log_path = tmp_path / "failed.log"
    diagnostic = tmp_path / "static_extension_init_failure.json"
    conn = state._connect(db)
    scheduling._insert_run(
        conn,
        run_id="failed-run",
        logical_id="pact-witness-acceptance",
        reason="prove NumPy wrapped module-exec diagnosis",
        command=[sys.executable, "-c", "print('fail')"],
        cwd=state.ROOT,
        resource_family="wasm-browser",
        contention_key="wasm:pact-witness",
        scopes=["tools/proof_queue.py"],
        git_snapshot={
            "available": True,
            "head": "abc123",
            "dirty": False,
            "status": [],
        },
        log_path=log_path,
        summary_json=tmp_path / "failed.memory_guard.json",
    )
    log_path.write_text(
        "Error: Unhandled Molt exception: ImportError:\n\n"
        "IMPORTANT: PLEASE READ THIS FOR ADVICE ON HOW TO SOLVE THIS ISSUE!\n\n"
        "Original error was: _multiarray_umath: static-link PyModuleDef "
        "Py_mod_exec slot returned non-zero without setting an exception "
        "(last silent C-API failure: PyType_Ready(null type))\n"
        f"  diagnostic_json={diagnostic}\n"
        "subprocess.CalledProcessError: Command '['node', 'wasm/run_wasm.js']' "
        "returned non-zero exit status 1.\n",
        encoding="utf-8",
    )
    state._update_run(conn, "failed-run", status="failed", returncode=1)

    assert (
        cli.main(
            [
                "--db",
                str(db),
                "--logs-root",
                str(tmp_path / "runs"),
                "--repo-root",
                str(state.ROOT),
                "evidence",
                "--run-id",
                "failed-run",
            ]
        )
        == 0
    )
    evidence = json.loads(capsys.readouterr().out)
    diagnostics = evidence[0]["diagnostics"]
    assert diagnostics[0]["signal_id"] == "static-pymodexec-nonzero"
    assert "_multiarray_umath" in diagnostics[0]["summary"]
    assert "PyType_Ready(null type)" in diagnostics[0]["evidence"]
    assert diagnostics[0]["artifacts"] == [str(diagnostic)]
    assert "python-exception" not in {item["signal_id"] for item in diagnostics}


def test_proof_queue_diagnoses_pact_witness_fixture_missing(
    tmp_path: Path, capsys: pytest.CaptureFixture[str]
) -> None:
    db = tmp_path / "proof_queue.sqlite3"
    log_path = tmp_path / "pact-fixture-missing.log"
    conn = state._connect(db)
    scheduling._insert_run(
        conn,
        run_id="pact-fixture-missing-run",
        logical_id="pact-witness-acceptance",
        reason="prove missing Pact fixture diagnosis",
        command=[sys.executable, "tools/pact_witness_acceptance.py"],
        cwd=state.ROOT,
        resource_family="wasm-browser",
        contention_key="wasm:pact-witness",
        scopes=["tools/pact_witness_acceptance.py"],
        git_snapshot={
            "available": True,
            "head": "abc123",
            "dirty": False,
            "status": [],
        },
        log_path=log_path,
        summary_json=tmp_path / "pact-fixture-missing.memory_guard.json",
    )
    log_path.write_text(
        "Successfully built tmp/pact_witness_acceptance_queue/build/output.wasm\n"
        "Successfully linked tmp/pact_witness_acceptance_queue/build/output_linked.wasm\n"
        "missing Pact fixture: collab/pact/pact_witness_kernel/lstar_sample.npz\n",
        encoding="utf-8",
    )
    state._update_run(
        conn,
        "pact-fixture-missing-run",
        status="failed",
        returncode=1,
    )

    assert (
        cli.main(
            [
                "--db",
                str(db),
                "--logs-root",
                str(tmp_path / "runs"),
                "--repo-root",
                str(state.ROOT),
                "evidence",
                "--run-id",
                "pact-fixture-missing-run",
            ]
        )
        == 0
    )
    evidence = json.loads(capsys.readouterr().out)
    diagnostics = evidence[0]["diagnostics"]
    assert diagnostics[0]["signal_id"] == "pact-witness-fixture-missing"
    assert (
        "fixture/reference oracle inside the run directory"
        in diagnostics[0]["next_action"]
    )
    assert "unclassified-failed-proof" not in {
        item["signal_id"] for item in diagnostics
    }


def test_proof_queue_diagnoses_rust_compile_error_and_guard_orphan_cleanup(
    tmp_path: Path, capsys: pytest.CaptureFixture[str]
) -> None:
    db = tmp_path / "proof_queue.sqlite3"
    log_path = tmp_path / "rust-failed.log"
    conn = state._connect(db)
    scheduling._insert_run(
        conn,
        run_id="rust-failed-run",
        logical_id="rust-failed",
        reason="prove Rust compiler diagnostics",
        command=["cargo", "test", "-p", "molt-runtime"],
        cwd=state.ROOT,
        resource_family="rust",
        contention_key="rust:molt-runtime",
        scopes=["runtime/molt-runtime/src/cpython_abi_hooks.rs"],
        git_snapshot={
            "available": True,
            "head": "abc123",
            "dirty": False,
            "status": [],
        },
        log_path=log_path,
        summary_json=tmp_path / "rust-failed.memory_guard.json",
    )
    state._insert_note(
        conn,
        run_id="rust-failed-run",
        body="test: capture rustc and memory guard signals",
        kind="submission",
        author="codex",
    )
    log_path.write_text(
        "\n".join(
            [
                "error[E0308]: mismatched types",
                "error: could not compile `molt-runtime` (lib test) due to 1 previous error",
                "memory_guard: orphaned child processes detected after command exit; killed_at=2026-07-01T23:21:47Z elapsed=20.83s",
                "memory_guard: quarantined Cargo incremental state after orphaned_processes_cleaned: moved=1 target_dir=E:\\Molt\\target\\sessions\\proof-rust quarantine_dir=E:\\Molt\\target\\sessions\\proof-rust\\.molt_state\\quarantine\\cargo_incremental\\20260703-053414-pid5988-orphaned_processes_cleaned receipt=E:\\Molt target\\sessions\\proof-rust\\.molt_state\\quarantine\\cargo_incremental\\20260703-053414-pid5988-orphaned_processes_cleaned\\receipt.json errors=1",
            ]
        ),
        encoding="utf-8",
    )
    state._update_run(conn, "rust-failed-run", status="failed", returncode=101)

    assert (
        cli.main(
            [
                "--db",
                str(db),
                "--logs-root",
                str(tmp_path / "runs"),
                "--repo-root",
                str(state.ROOT),
                "evidence",
                "--run-id",
                "rust-failed-run",
            ]
        )
        == 0
    )
    evidence = json.loads(capsys.readouterr().out)
    quarantine_receipt = (
        "E:\\Molt target\\sessions\\proof-rust\\.molt_state\\quarantine\\"
        "cargo_incremental\\20260703-053414-pid5988-orphaned_processes_cleaned\\"
        "receipt.json"
    )
    signals = [item["signal_id"] for item in evidence[0]["diagnostics"]]
    assert signals[:2] == ["rust-compiler-error", "memory-guard-orphan-cleanup"]
    orphan_diagnostic = evidence[0]["diagnostics"][1]
    assert quarantine_receipt in orphan_diagnostic["evidence"]
    assert orphan_diagnostic["artifacts"] == [quarantine_receipt]

    assert (
        cli.main(
            [
                "--db",
                str(db),
                "--logs-root",
                str(tmp_path / "runs"),
                "--repo-root",
                str(state.ROOT),
                "audit",
                "--json",
                "--no-notebook-check",
            ]
        )
        == 0
    )
    audit = json.loads(capsys.readouterr().out)
    orphan_issue = next(
        item
        for item in audit["issues"]
        if item["signal_id"] == "audit-memory-guard-orphan-cleanup"
    )
    assert orphan_issue["artifacts"] == [quarantine_receipt]


def test_proof_queue_diagnoses_rust_test_failure_before_cargo_error_line(
    tmp_path: Path, capsys: pytest.CaptureFixture[str]
) -> None:
    db = tmp_path / "proof_queue.sqlite3"
    log_path = tmp_path / "rust-test-failed.log"
    conn = state._connect(db)
    scheduling._insert_run(
        conn,
        run_id="rust-test-failed-run",
        logical_id="r3b-boxed-nonscalar-alias-regressions-warm-20260705",
        reason="prove cargo test failures are not compiler diagnostics",
        command=[
            "cargo",
            "test",
            "-p",
            "molt-tir",
            "representation_plan::tests::",
            "--lib",
        ],
        cwd=state.ROOT,
        resource_family="cargo",
        contention_key="cargo:molt-tir",
        scopes=["runtime/molt-tir/src/representation_plan/tests.rs"],
        git_snapshot={
            "available": True,
            "head": "abc123",
            "dirty": False,
            "status": [],
        },
        log_path=log_path,
        summary_json=tmp_path / "rust-test-failed.memory_guard.json",
    )
    state._insert_note(
        conn,
        run_id="rust-test-failed-run",
        body="test: cargo test assertion failure must not masquerade as rustc",
        kind="submission",
        author="codex",
    )
    log_path.write_text(
        "\n".join(
            [
                "   Compiling molt-tir v0.1.0 (C:\\repo\\runtime\\molt-tir)",
                "    Finished `test` profile [unoptimized + debuginfo] target(s) in 18.67s",
                "     Running unittests src\\lib.rs (D:\\Molt\\target\\debug\\deps\\molt_tir.exe)",
                "running 2 tests",
                "test representation_plan::tests::safe_case ... ok",
                "test representation_plan::tests::boxed_abi_parameters_survive_synthetic_self_stores ... FAILED",
                "",
                "failures:",
                "    representation_plan::tests::boxed_abi_parameters_survive_synthetic_self_stores",
                "",
                "test result: FAILED. 1 passed; 1 failed; 0 ignored; 0 measured; 142 filtered out; finished in 0.30s",
                "",
                "error: test failed, to rerun pass `-p molt-tir --lib`",
            ]
        ),
        encoding="utf-8",
    )
    state._update_run(conn, "rust-test-failed-run", status="failed", returncode=101)

    assert (
        cli.main(
            [
                "--db",
                str(db),
                "--logs-root",
                str(tmp_path / "runs"),
                "--repo-root",
                str(state.ROOT),
                "evidence",
                "--run-id",
                "rust-test-failed-run",
            ]
        )
        == 0
    )
    evidence = json.loads(capsys.readouterr().out)
    diagnostics = evidence[0]["diagnostics"]
    signals = [item["signal_id"] for item in diagnostics]
    assert signals[0] == "rust-test-failure"
    assert "rust-compiler-error" not in signals
    assert "test result: FAILED. 1 passed; 1 failed" in diagnostics[0]["evidence"]
    assert (
        "error: test failed, to rerun pass `-p molt-tir --lib`"
        in diagnostics[0]["evidence"]
    )
    assert (
        "representation_plan::tests::boxed_abi_parameters_survive_synthetic_self_stores"
        in diagnostics[0]["evidence"]
    )


def test_proof_queue_diagnoses_nested_guarded_exec_orphan_cleanup(
    tmp_path: Path, capsys: pytest.CaptureFixture[str]
) -> None:
    db = tmp_path / "proof_queue.sqlite3"
    log_path = tmp_path / "nested-guarded-exec.log"
    conn = state._connect(db)
    scheduling._insert_run(
        conn,
        run_id="nested-guarded-exec-run",
        logical_id="nested-guarded-exec",
        reason="prove nested guarded_exec orphan cleanup diagnosis",
        command=["cargo", "test", "-p", "molt-passes", "--lib"],
        cwd=state.ROOT,
        resource_family="rust",
        contention_key="rust:molt-passes",
        scopes=["runtime/molt-passes/src/representation_facts.rs"],
        git_snapshot={
            "available": True,
            "head": "abc123",
            "dirty": False,
            "status": [],
        },
        log_path=log_path,
        summary_json=tmp_path / "nested-guarded-exec.memory_guard.json",
    )
    state._insert_note(
        conn,
        run_id="nested-guarded-exec-run",
        body="test: nested guarded_exec orphan cleanup remains distinct",
        kind="submission",
        author="codex",
    )
    receipt = (
        "E:\\Molt\\target\\sessions\\proof-rust\\.molt_state\\quarantine\\"
        "cargo_incremental\\20260703-030911-pid17104-orphaned_processes_cleaned\\"
        "receipt.json"
    )
    log_path.write_text(
        "\n".join(
            [
                "memory_guard: MOLT_TEST_SUITE guarded command: cargo test -p molt-passes --lib",
                "memory_guard: quarantined Cargo incremental state after orphaned_processes_cleaned: moved=1 target_dir=E:\\Molt\\target\\sessions\\proof-rust quarantine_dir=E:\\Molt\\target\\sessions\\proof-rust\\.molt_state\\quarantine\\cargo_incremental\\20260703-030911-pid17104-orphaned_processes_cleaned receipt="
                + receipt
                + " errors=0",
                "memory_guard: orphaned child processes detected after command exit; killed_at=2026-07-03T03:09:11Z elapsed=225.28s pgids=8176 reason=direct child exited while descendants were still live",
                "guarded_exec: elapsed=225.28s returncode=0 command=cargo test -p molt-passes --lib",
            ]
        ),
        encoding="utf-8",
    )
    state._update_run(conn, "nested-guarded-exec-run", status="passed", returncode=0)

    assert (
        cli.main(
            [
                "--db",
                str(db),
                "--logs-root",
                str(tmp_path / "runs"),
                "--repo-root",
                str(state.ROOT),
                "evidence",
                "--run-id",
                "nested-guarded-exec-run",
            ]
        )
        == 0
    )
    evidence = json.loads(capsys.readouterr().out)
    diagnostics = evidence[0]["diagnostics"]
    assert diagnostics[0]["signal_id"] == "nested-memory-guard-orphan-cleanup"
    assert diagnostics[0]["severity"] == "warning"
    assert "Nested guarded_exec" in diagnostics[0]["summary"]
    assert diagnostics[0]["artifacts"] == [receipt]

    assert (
        cli.main(
            [
                "--db",
                str(db),
                "--logs-root",
                str(tmp_path / "runs"),
                "--repo-root",
                str(state.ROOT),
                "audit",
                "--json",
                "--no-notebook-check",
            ]
        )
        == 0
    )
    audit = json.loads(capsys.readouterr().out)
    nested_issue = next(
        item
        for item in audit["issues"]
        if item["signal_id"] == "audit-nested-memory-guard-orphan-cleanup"
    )
    assert nested_issue["artifacts"] == [receipt]


def test_proof_queue_diagnoses_memory_guard_timeout_before_orphan_cleanup(
    tmp_path: Path, capsys: pytest.CaptureFixture[str]
) -> None:
    db = tmp_path / "proof_queue.sqlite3"
    log_path = tmp_path / "timeout.log"
    summary_path = tmp_path / "timeout.memory_guard.json"
    nodeid = "tests/tools/test_proof_queue.py::test_generic_timeout_context"
    conn = state._connect(db)
    scheduling._insert_run(
        conn,
        run_id="timeout-run",
        logical_id="generic-timeout-recheck",
        reason="prove timeout diagnosis outranks orphan cleanup",
        command=[
            sys.executable,
            "-m",
            "pytest",
            "tests/tools/test_proof_queue.py",
        ],
        cwd=state.ROOT,
        resource_family="python-tests",
        contention_key="python-tests",
        scopes=["tools/proof_queue.py"],
        git_snapshot={
            "available": True,
            "head": "abc123",
            "dirty": False,
            "status": [],
        },
        log_path=log_path,
        summary_json=summary_path,
    )
    state._insert_note(
        conn,
        run_id="timeout-run",
        body="test: timeout must be primary queue evidence",
        kind="submission",
        author="codex",
    )
    log_path.write_text(
        "\n".join(
            [
                "F.",
                "[FAIL] tests\\differential\\stdlib\\sys_metadata_intrinsics.py "
                "(native) mismatch: stdout mismatch; exit code ref=1 cand=0",
                "  CPython stdout: ''",
                "  Molt    stdout: 'ok\\n'",
                "memory_guard: timeout after 900.00s; terminated tracked process tree to prevent orphaned Molt subprocesses: killed_at=2026-07-02T20:04:56Z elapsed=901.44s child_pid=33900",
                "memory_guard: orphaned child processes detected after command exit; terminated tracked process groups to prevent accumulation: killed_at=2026-07-02T20:04:56Z elapsed=901.44s pgids=18104 reason=direct child exited while descendants were still live",
            ]
        ),
        encoding="utf-8",
    )
    summary_path.write_text(
        json.dumps(
            {
                "repro": {
                    "pytest": {
                        "current_test_file": {
                            "payload": {
                                "nodeid": nodeid,
                                "phase": "call",
                            }
                        }
                    }
                }
            }
        ),
        encoding="utf-8",
    )
    state._update_run(conn, "timeout-run", status="failed", returncode=124)

    assert (
        cli.main(
            [
                "--db",
                str(db),
                "--logs-root",
                str(tmp_path / "runs"),
                "--repo-root",
                str(state.ROOT),
                "evidence",
                "--run-id",
                "timeout-run",
            ]
        )
        == 0
    )
    evidence = json.loads(capsys.readouterr().out)
    diagnostics = evidence[0]["diagnostics"]
    assert [item["signal_id"] for item in diagnostics[:2]] == [
        "memory-guard-timeout",
        "memory-guard-orphan-cleanup",
    ]
    assert "900.00s" in diagnostics[0]["summary"]
    assert "test_generic_timeout_context" in diagnostics[0]["summary"]
    assert "pytest_phase=call" in diagnostics[0]["evidence"]
    assert f"Inspect {nodeid} once" in diagnostics[0]["next_action"]
    assert diagnostics[0]["artifacts"] == [str(summary_path), str(log_path)]
    assert "molt-diff-output-mismatch" not in {
        item["signal_id"] for item in diagnostics
    }


def test_proof_queue_routes_native_import_bootstrap_timeout_to_r1_owner(
    tmp_path: Path, capsys: pytest.CaptureFixture[str]
) -> None:
    db = tmp_path / "proof_queue.sqlite3"
    log_path = tmp_path / "native-import-timeout.log"
    summary_path = tmp_path / "native-import-timeout.memory_guard.json"
    nodeid = (
        "tests/test_native_import_bootstrap_regressions.py::"
        "test_native_package_entry_direct_import_and_from_import_bindings_are_resolved"
    )
    conn = state._connect(db)
    scheduling._insert_run(
        conn,
        run_id="native-import-timeout-run",
        logical_id="native-import-bootstrap-regressions-full",
        reason="prove native call-lane timeouts route to the lane owner",
        command=[
            sys.executable,
            "-m",
            "pytest",
            "tests/test_native_import_bootstrap_regressions.py",
        ],
        cwd=state.ROOT,
        resource_family="python-tests",
        contention_key="native-import-regression",
        scopes=["tests/test_native_import_bootstrap_regressions.py"],
        git_snapshot={
            "available": True,
            "head": "abc123",
            "dirty": False,
            "status": [],
        },
        log_path=log_path,
        summary_json=summary_path,
    )
    log_path.write_text(
        (
            "memory_guard: timeout after 7200.00s; terminated tracked process "
            "tree to prevent orphaned Molt subprocesses\n"
        ),
        encoding="utf-8",
    )
    summary_path.write_text(
        json.dumps(
            {
                "repro": {
                    "pytest": {
                        "current_test_file": {
                            "payload": {
                                "nodeid": nodeid,
                                "phase": "call",
                            }
                        }
                    }
                }
            }
        ),
        encoding="utf-8",
    )
    state._update_run(
        conn, "native-import-timeout-run", status="failed", returncode=124
    )

    assert (
        cli.main(
            [
                "--db",
                str(db),
                "--logs-root",
                str(tmp_path / "runs"),
                "--repo-root",
                str(state.ROOT),
                "evidence",
                "--run-id",
                "native-import-timeout-run",
            ]
        )
        == 0
    )
    evidence = json.loads(capsys.readouterr().out)
    diagnostics = evidence[0]["diagnostics"]
    assert diagnostics[0]["signal_id"] == "native-call-lane-memory-guard-timeout"
    assert diagnostics[0]["severity"] == "error"
    assert "R1 integrator" in diagnostics[0]["summary"]
    assert "pytest_phase=call" in diagnostics[0]["evidence"]
    assert (
        "Route this timeout row to the native call-lane owner"
        in diagnostics[0]["next_action"]
    )
    assert "runtime/molt-runtime/src/call/function.rs" in diagnostics[0]["scopes"]
    assert diagnostics[0]["artifacts"] == [str(summary_path), str(log_path)]
    assert "memory-guard-timeout" not in {item["signal_id"] for item in diagnostics}

    assert (
        cli.main(
            [
                "--db",
                str(db),
                "--logs-root",
                str(tmp_path / "runs"),
                "--repo-root",
                str(state.ROOT),
                "audit",
                "--no-notebook-check",
            ]
        )
        == 1
    )
    audit_out = capsys.readouterr().out
    assert "diagnostics: native-call-lane-memory-guard-timeout=1" in audit_out
    assert (
        "audit-native-call-lane-memory-guard-timeout run=native-import-timeout-run"
        in audit_out
    )

    assert (
        cli.main(
            [
                "--db",
                str(db),
                "--logs-root",
                str(tmp_path / "runs"),
                "--repo-root",
                str(state.ROOT),
                "status",
                "--recent",
                "1",
            ]
        )
        == 0
    )
    status_out = capsys.readouterr().out
    assert "diagnosis=native-call-lane-memory-guard-timeout" in status_out
    assert f"artifacts={summary_path}, {log_path}" in status_out


def test_proof_queue_diagnoses_pytest_assertion_failure(
    tmp_path: Path, capsys: pytest.CaptureFixture[str]
) -> None:
    db = tmp_path / "proof_queue.sqlite3"
    log_path = tmp_path / "pytest-failed.log"
    conn = state._connect(db)
    scheduling._insert_run(
        conn,
        run_id="pytest-failed-run",
        logical_id="pytest-failed",
        reason="prove pytest diagnostics",
        command=[sys.executable, "-m", "pytest", "tests/test_wasm_link_validation.py"],
        cwd=state.ROOT,
        resource_family="python",
        contention_key="python:pytest-failed",
        scopes=["tests/test_wasm_link_validation.py"],
        git_snapshot={
            "available": True,
            "head": "abc123",
            "dirty": False,
            "status": [],
        },
        log_path=log_path,
        summary_json=tmp_path / "pytest-failed.memory_guard.json",
    )
    state._insert_note(
        conn,
        run_id="pytest-failed-run",
        body="test: capture pytest assertion diagnostics",
        kind="submission",
        author="codex",
    )
    log_path.write_text(
        "\n".join(
            [
                "FAILED tests/test_wasm_link_validation.py::test_split_runtime_app_materialization_declares_code_ref_funcs",
                "E   AssertionError: unexpected rescan",
                "1 failed, 3 passed",
            ]
        ),
        encoding="utf-8",
    )
    state._update_run(conn, "pytest-failed-run", status="failed", returncode=1)

    assert (
        cli.main(
            [
                "--db",
                str(db),
                "--logs-root",
                str(tmp_path / "runs"),
                "--repo-root",
                str(state.ROOT),
                "evidence",
                "--run-id",
                "pytest-failed-run",
            ]
        )
        == 0
    )
    evidence = json.loads(capsys.readouterr().out)
    diagnostics = evidence[0]["diagnostics"]
    assert diagnostics[0]["signal_id"] == "pytest-failure"
    assert "test_split_runtime_app_materialization_declares_code_ref_funcs" in str(
        diagnostics[0]["summary"]
    )
    assert "unexpected rescan" in str(diagnostics[0]["evidence"])


def test_proof_queue_routes_native_import_bootstrap_pytest_failure_to_r1_owner(
    tmp_path: Path, capsys: pytest.CaptureFixture[str]
) -> None:
    db = tmp_path / "proof_queue.sqlite3"
    log_path = tmp_path / "native-import-bootstrap.log"
    nodeid = (
        "tests/test_native_import_bootstrap_regressions.py::"
        "test_native_imported_module_dunder_getattr_handles_missing_attr"
    )
    conn = state._connect(db)
    scheduling._insert_run(
        conn,
        run_id="native-import-bootstrap-run",
        logical_id="indirect-call-trampoline-fix-e2e-shard",
        reason="prove native call-lane pytest failures route to the lane owner",
        command=[
            sys.executable,
            "-m",
            "pytest",
            "tests/test_native_import_bootstrap_regressions.py",
        ],
        cwd=state.ROOT,
        resource_family="python-tests",
        contention_key="native-import-regression",
        scopes=["tests/test_native_import_bootstrap_regressions.py"],
        git_snapshot={
            "available": True,
            "head": "abc123",
            "dirty": False,
            "status": [],
        },
        log_path=log_path,
        summary_json=tmp_path / "native-import-bootstrap.memory_guard.json",
    )
    log_path.write_text(
        "\n".join(
            [
                f"FAILED {nodeid}",
                "E   AssertionError: SystemError: module id out of range",
                "1 failed, 146 passed",
            ]
        ),
        encoding="utf-8",
    )
    state._update_run(
        conn, "native-import-bootstrap-run", status="failed", returncode=1
    )

    assert (
        cli.main(
            [
                "--db",
                str(db),
                "--logs-root",
                str(tmp_path / "runs"),
                "--repo-root",
                str(state.ROOT),
                "evidence",
                "--run-id",
                "native-import-bootstrap-run",
            ]
        )
        == 0
    )
    evidence = json.loads(capsys.readouterr().out)
    diagnostics = evidence[0]["diagnostics"]
    assert diagnostics[0]["signal_id"] == "native-call-lane-pytest-failure"
    assert diagnostics[0]["severity"] == "error"
    assert "R1 integrator" in diagnostics[0]["summary"]
    assert "module id out of range" in diagnostics[0]["evidence"]
    assert (
        "Route this row to the native call-lane owner" in diagnostics[0]["next_action"]
    )
    assert "runtime/molt-runtime/src/call/function.rs" in diagnostics[0]["scopes"]
    assert "pytest-failure" not in {item["signal_id"] for item in diagnostics}


def test_proof_queue_diagnoses_cold_single_cargo_proof_policy_refusal(
    tmp_path: Path, capsys: pytest.CaptureFixture[str]
) -> None:
    db = tmp_path / "proof_queue.sqlite3"
    log_path = tmp_path / "cold-single-cargo.log"
    conn = state._connect(db)
    scheduling._insert_run(
        conn,
        run_id="cold-single-cargo-run",
        logical_id="indirect-call-trampoline-fix-runtime-call-shard",
        reason="prove cold single Cargo proof policy diagnostics",
        command=[
            "uv",
            "run",
            "--active",
            "--project",
            ".",
            "--python",
            "3.12",
            "python",
            "tools/guarded_exec.py",
            "--prefix",
            "MOLT_TEST_SUITE",
            "--",
            "cargo",
            "test",
            "-p",
            "molt-runtime",
            "--lib",
            "call",
        ],
        cwd=state.ROOT,
        resource_family="cargo",
        contention_key="cargo:molt-runtime",
        scopes=["tools/proof_queue.py"],
        git_snapshot={
            "available": True,
            "head": "abc123",
            "dirty": False,
            "status": [],
        },
        log_path=log_path,
        summary_json=tmp_path / "cold-single-cargo.memory_guard.json",
    )
    state._insert_note(
        conn,
        run_id="cold-single-cargo-run",
        body="test: cold single-test Cargo policy refusal must be classified",
        kind="submission",
        author="codex",
    )
    log_path.write_text(
        "proof queue refuses cold-prone single-test Cargo proofs "
        "('call' under --lib). Batch the relevant crate shard in one compile.\n",
        encoding="utf-8",
    )
    state._update_run(
        conn,
        "cold-single-cargo-run",
        status="failed",
        returncode=2,
    )

    assert (
        cli.main(
            [
                "--db",
                str(db),
                "--logs-root",
                str(tmp_path / "runs"),
                "--repo-root",
                str(state.ROOT),
                "evidence",
                "--run-id",
                "cold-single-cargo-run",
            ]
        )
        == 0
    )
    evidence = json.loads(capsys.readouterr().out)
    diagnostics = evidence[0]["diagnostics"]
    assert diagnostics[0]["signal_id"] == "queue-cold-single-cargo-proof"
    assert diagnostics[0]["severity"] == "operator"
    assert "filter call" in diagnostics[0]["summary"]
    assert "--allow-warm-single-test" in diagnostics[0]["next_action"]


def test_proof_queue_diagnoses_molt_runtime_invalid_object_header(
    tmp_path: Path, capsys: pytest.CaptureFixture[str]
) -> None:
    db = tmp_path / "proof_queue.sqlite3"
    log_path = tmp_path / "molt-runtime-invalid-header.log"
    conn = state._connect(db)
    scheduling._insert_run(
        conn,
        run_id="molt-runtime-invalid-header-run",
        logical_id="native-module-dunder-cleanup-trace",
        reason="prove Molt runtime fatal diagnostics",
        command=[str(tmp_path / "compiled-native-binary")],
        cwd=state.ROOT,
        resource_family="python-tests",
        contention_key="native-import-regression",
        scopes=["runtime/molt-runtime/src/builtins/modules.rs"],
        git_snapshot={
            "available": True,
            "head": "abc123",
            "dirty": False,
            "status": [],
        },
        log_path=log_path,
        summary_json=tmp_path / "molt-runtime-invalid-header.memory_guard.json",
    )
    state._insert_note(
        conn,
        run_id="molt-runtime-invalid-header-run",
        body="test: runtime fatal must be classified before generic pytest failure",
        kind="submission",
        author="codex",
    )
    log_path.write_text(
        "\n".join(
            [
                "module_get_attr: mod=0x7ffc013480238fb8 "
                "attr=0x7ffc013480230dc8 name=__dict__",
                "molt module attr get module=probe_mod attr=__dict__",
                "molt fatal: invalid object header in dec_ref "
                "ptr=0x134802399a8 type_id=2149817200 "
                "(use-after-free or corrupted header)",
                "FAILED tests/test_native_import_bootstrap_regressions.py::"
                "test_native_imported_module_dunder_getattr_handles_missing_attr",
            ]
        ),
        encoding="utf-8",
    )
    state._update_run(
        conn,
        "molt-runtime-invalid-header-run",
        status="failed",
        returncode=4294967295,
    )

    assert (
        cli.main(
            [
                "--db",
                str(db),
                "--logs-root",
                str(tmp_path / "runs"),
                "--repo-root",
                str(state.ROOT),
                "evidence",
                "--run-id",
                "molt-runtime-invalid-header-run",
            ]
        )
        == 0
    )
    evidence = json.loads(capsys.readouterr().out)
    diagnostics = evidence[0]["diagnostics"]
    assert diagnostics[0]["signal_id"] == "molt-runtime-invalid-object-header"
    assert diagnostics[0]["severity"] == "error"
    assert "dec_ref" in diagnostics[0]["summary"]
    assert "use-after-free or corrupted header" in diagnostics[0]["evidence"]
    assert "runtime/molt-runtime/" in diagnostics[0]["scopes"]
    assert "unclassified-failed-proof" not in {
        item["signal_id"] for item in diagnostics
    }


def test_proof_queue_diagnoses_perf_scoreboard_not_quiescent(
    tmp_path: Path, capsys: pytest.CaptureFixture[str]
) -> None:
    db = tmp_path / "proof_queue.sqlite3"
    log_path = tmp_path / "perf-scoreboard-not-quiescent.log"
    summary_json = tmp_path / "perf-scoreboard-not-quiescent.memory_guard.json"
    conn = state._connect(db)
    scheduling._insert_run(
        conn,
        run_id="perf-scoreboard-not-quiescent-run",
        logical_id="c4-current-perf-scoreboard",
        reason="prove perf scoreboard quiescence diagnostic",
        command=[
            "uv",
            "run",
            "--active",
            "--project",
            ".",
            "--python",
            "3.12",
            "python",
            "tools\\perf_scoreboard.py",
            "--require-quiescent",
        ],
        cwd=state.ROOT,
        resource_family="perf",
        contention_key="c4-current-perf-scoreboard",
        scopes=["tools/perf_scoreboard.py"],
        git_snapshot={
            "available": True,
            "head": "abc123",
            "dirty": False,
            "status": [],
        },
        log_path=log_path,
        summary_json=summary_json,
    )
    log_path.write_text(
        "\n".join(
            [
                "[scoreboard] waiting for quiescence (sample 12, next check in "
                "15s, budget left 15s): 3 active build process(es) "
                "(cargo/rustc/molt-backend/molt build); 1-min load 24.00 > "
                "threshold 12.00 (ncpu=24 * 0.5)",
                "[scoreboard] machine NOT quiescent - 3 active build process(es) "
                "(cargo/rustc/molt-backend/molt build); 1-min load 22.32 > "
                "threshold 12.00 (ncpu=24 * 0.5)",
                "[scoreboard] *** NON-AUTHORITATIVE: machine not quiet; do not "
                "optimize from this red list (EXPLORATORY only) ***",
                "[scoreboard]     reason: machine NOT quiescent "
                "(--require-quiescent): 3 active build process(es)",
                "[scoreboard] refusing non-authoritative measurement before "
                "starting benchmark builds",
                "proof_queue finished status=failed exit_code=1 elapsed=361.359s",
            ]
        ),
        encoding="utf-8",
    )
    state._update_run(
        conn,
        "perf-scoreboard-not-quiescent-run",
        status="failed",
        returncode=1,
    )

    assert (
        cli.main(
            [
                "--db",
                str(db),
                "--logs-root",
                str(tmp_path / "runs"),
                "--repo-root",
                str(state.ROOT),
                "evidence",
                "--run-id",
                "perf-scoreboard-not-quiescent-run",
            ]
        )
        == 0
    )
    evidence = json.loads(capsys.readouterr().out)
    diagnostics = evidence[0]["diagnostics"]
    assert diagnostics[0]["signal_id"] == "perf-scoreboard-not-quiescent"
    assert diagnostics[0]["severity"] == "operator"
    assert "failed closed before benchmarking" in diagnostics[0]["summary"]
    assert "machine NOT quiescent" in diagnostics[0]["evidence"]
    assert "refusing non-authoritative measurement" in diagnostics[0]["evidence"]
    assert "--allow-nonauthoritative" in diagnostics[0]["next_action"]
    assert "tools/perf_scoreboard.py" in diagnostics[0]["scopes"]
    assert diagnostics[0]["artifacts"] == [str(summary_json), str(log_path)]
    assert "unclassified-failed-proof" not in {
        item["signal_id"] for item in diagnostics
    }


def test_proof_queue_diagnoses_runtime_wasm_rust_target_missing(
    tmp_path: Path, capsys: pytest.CaptureFixture[str]
) -> None:
    db = tmp_path / "proof_queue.sqlite3"
    log_path = tmp_path / "runtime-wasm-rust-target.log"
    conn = state._connect(db)
    scheduling._insert_run(
        conn,
        run_id="runtime-wasm-rust-target-run",
        logical_id="pact-witness-acceptance",
        reason="prove missing wasm Rust target diagnosis",
        command=[sys.executable, "tools/pact_witness_acceptance.py"],
        cwd=state.ROOT,
        resource_family="wasm-browser",
        contention_key="wasm:pact",
        scopes=["tools/pact_witness_acceptance.py"],
        git_snapshot={
            "available": True,
            "head": "abc123",
            "dirty": False,
            "status": [],
        },
        log_path=log_path,
        summary_json=tmp_path / "runtime-wasm-rust-target.memory_guard.json",
    )
    state._insert_note(
        conn,
        run_id="runtime-wasm-rust-target-run",
        body="test: missing Rust target must surface as actionable audit DX",
        kind="submission",
        author="codex",
    )
    log_path.write_text(
        "Runtime wasm build requires Rust target wasm32-wasip1, but the active "
        "Rust toolchain does not provide it. Run: rustup target add "
        "wasm32-wasip1 --toolchain 1.96.1\n"
        "Runtime wasm build failed\n"
        "subprocess.CalledProcessError: Command '['python', '-m', 'molt', "
        "'build']' returned non-zero exit status 2.\n",
        encoding="utf-8",
    )
    state._update_run(
        conn, "runtime-wasm-rust-target-run", status="failed", returncode=1
    )

    assert (
        cli.main(
            [
                "--db",
                str(db),
                "--logs-root",
                str(tmp_path / "runs"),
                "--repo-root",
                str(state.ROOT),
                "evidence",
                "--run-id",
                "runtime-wasm-rust-target-run",
            ]
        )
        == 0
    )
    evidence = json.loads(capsys.readouterr().out)
    diagnostics = evidence[0]["diagnostics"]
    assert diagnostics[0]["signal_id"] == "runtime-wasm-rust-target-missing"
    assert diagnostics[0]["severity"] == "infra"
    assert "wasm32-wasip1" in diagnostics[0]["summary"]
    assert "rustup target add wasm32-wasip1" in diagnostics[0]["evidence"]
    assert diagnostics[0]["artifacts"] == [
        str(tmp_path / "runtime-wasm-rust-target.memory_guard.json"),
        str(log_path),
    ]
    assert "src/molt/cli/wasm_toolchain.py" in diagnostics[0]["scopes"]
    assert "tools/wasm_toolchain.py" not in diagnostics[0]["scopes"]
    assert "python-exception" not in {item["signal_id"] for item in diagnostics}
    assert "unclassified-failed-proof" not in {
        item["signal_id"] for item in diagnostics
    }

    assert (
        cli.main(
            [
                "--db",
                str(db),
                "--logs-root",
                str(tmp_path / "runs"),
                "--repo-root",
                str(state.ROOT),
                "audit",
                "--no-notebook-check",
            ]
        )
        == 0
    )
    audit_out = capsys.readouterr().out
    assert "diagnostics: runtime-wasm-rust-target-missing=1" in audit_out
    assert "issue_severity: warning=1" in audit_out
    assert (
        "audit-runtime-wasm-rust-target-missing run=runtime-wasm-rust-target-run"
        in audit_out
    )
    assert str(log_path) in audit_out


def test_proof_queue_diagnoses_wasm_toolchain_contract_import_missing(
    monkeypatch: pytest.MonkeyPatch,
    tmp_path: Path,
    capsys: pytest.CaptureFixture[str],
) -> None:
    db = tmp_path / "proof_queue.sqlite3"
    logs = tmp_path / "runs"
    command_marker = tmp_path / "proof-command-ran.txt"

    def fail_toolchain_import():
        raise ModuleNotFoundError("No module named 'packaging.specifiers'")

    monkeypatch.setattr(policy, "_load_wasm_toolchain", fail_toolchain_import)

    rc = cli.main(
        [
            "--db",
            str(db),
            "--logs-root",
            str(logs),
            "--repo-root",
            str(state.ROOT),
            "exec",
            "--id",
            "wasm-contract-import-missing-run",
            "--reason",
            "prove wasm preflight import-missing diagnosis",
            "--resource-family",
            "wasm-browser",
            "--contention-key",
            "wasm:contract-import-missing",
            "--",
            sys.executable,
            "-c",
            "from pathlib import Path; import sys; Path(sys.argv[1]).write_text('ran')",
            str(command_marker),
        ]
    )
    assert rc == 2
    assert not command_marker.exists()
    rows = _rows(db)
    assert rows[0]["status"] == "failed"
    assert rows[0]["returncode"] == 2
    assert rows[0]["logical_id"] == "wasm-contract-import-missing-run"
    run_id = str(rows[0]["run_id"])
    log_path = Path(rows[0]["log_path"])
    assert "failed to import WASM toolchain contract" in log_path.read_text(
        encoding="utf-8"
    )
    capsys.readouterr()

    assert (
        cli.main(
            [
                "--db",
                str(db),
                "--logs-root",
                str(tmp_path / "runs"),
                "--repo-root",
                str(state.ROOT),
                "evidence",
                "--run-id",
                run_id,
            ]
        )
        == 0
    )
    evidence = json.loads(capsys.readouterr().out)
    diagnostics = evidence[0]["diagnostics"]
    assert diagnostics[0]["signal_id"] == "wasm-toolchain-contract-import-missing"
    assert diagnostics[0]["severity"] == "infra"
    assert "packaging.specifiers" in diagnostics[0]["summary"]
    assert "active uv/project provisioning" in diagnostics[0]["next_action"]
    assert diagnostics[0]["artifacts"] == [
        str(rows[0]["summary_json"]),
        str(log_path),
    ]
    assert "python-exception" not in {item["signal_id"] for item in diagnostics}
    assert "unclassified-failed-proof" not in {
        item["signal_id"] for item in diagnostics
    }


def test_proof_queue_diagnoses_embedded_python_import_missing(
    tmp_path: Path, capsys: pytest.CaptureFixture[str]
) -> None:
    db = tmp_path / "proof_queue.sqlite3"
    log_path = tmp_path / "dx-build-missing-import.log"
    conn = state._connect(db)
    scheduling._insert_run(
        conn,
        run_id="dx-build-missing-import",
        logical_id="e2-build-wallclock",
        reason="prove build timer missing import diagnosis",
        command=[sys.executable, "tools/dx_build_timer.py"],
        cwd=state.ROOT,
        resource_family="native-build",
        contention_key="compiler-build-resource",
        scopes=["E2-BUILD-WALLCLOCK"],
        git_snapshot={
            "available": True,
            "head": "abc123",
            "dirty": False,
            "status": [],
        },
        log_path=log_path,
        summary_json=tmp_path / "dx-build-missing-import.memory_guard.json",
    )
    log_path.write_text(
        '"stderr_tail": "  File \\"target_python.py\\", line 10, in <module>'
        "\\nModuleNotFoundError: No module named 'packaging.specifiers'\",\n",
        encoding="utf-8",
    )
    state._update_run(conn, "dx-build-missing-import", status="failed", returncode=1)

    assert (
        cli.main(
            [
                "--db",
                str(db),
                "--logs-root",
                str(tmp_path / "runs"),
                "--repo-root",
                str(state.ROOT),
                "evidence",
                "--run-id",
                "dx-build-missing-import",
            ]
        )
        == 0
    )
    evidence = json.loads(capsys.readouterr().out)
    diagnostics = evidence[0]["diagnostics"]
    assert diagnostics[0]["signal_id"] == "proof-python-import-missing"
    assert diagnostics[0]["severity"] == "infra"
    assert "packaging.specifiers" in diagnostics[0]["summary"]
    assert "project-environment Python" in diagnostics[0]["next_action"]
    assert "unclassified-failed-proof" not in {
        item["signal_id"] for item in diagnostics
    }


def test_proof_queue_diagnoses_source_extension_nm_missing(
    tmp_path: Path, capsys: pytest.CaptureFixture[str]
) -> None:
    db = tmp_path / "proof_queue.sqlite3"
    log_path = tmp_path / "source-extension-nm.log"
    conn = state._connect(db)
    scheduling._insert_run(
        conn,
        run_id="source-extension-nm",
        logical_id="e1-numpy-multiarray-rebuild",
        reason="prove source-extension nm custody diagnosis",
        command=[sys.executable, "-m", "molt", "extension", "build"],
        cwd=state.ROOT,
        resource_family="wasm",
        contention_key="compiler-build-resource",
        scopes=["src/molt/cli/source_extensions.py"],
        git_snapshot={
            "available": True,
            "head": "abc123",
            "dirty": False,
            "status": [],
        },
        log_path=log_path,
        summary_json=tmp_path / "source-extension-nm.memory_guard.json",
    )
    log_path.write_text(
        "unable to read global symbol table for compiled extension object "
        "D:\\Molt\\tmp\\pact_numpy\\82_umathmodule.o; "
        "canonical LLVM/WASI nm authority is unavailable\n",
        encoding="utf-8",
    )
    state._update_run(conn, "source-extension-nm", status="failed", returncode=2)

    assert (
        cli.main(
            [
                "--db",
                str(db),
                "--logs-root",
                str(tmp_path / "runs"),
                "--repo-root",
                str(state.ROOT),
                "evidence",
                "--run-id",
                "source-extension-nm",
            ]
        )
        == 0
    )
    evidence = json.loads(capsys.readouterr().out)
    diagnostics = evidence[0]["diagnostics"]
    assert diagnostics[0]["signal_id"] == "source-extension-nm-missing"
    assert diagnostics[0]["severity"] == "infra"
    assert "82_umathmodule.o" in diagnostics[0]["summary"]
    assert "MOLT_TARGET_ROOT" in diagnostics[0]["next_action"]
    assert str(log_path) in diagnostics[0]["artifacts"]
    assert "unclassified-failed-proof" not in {
        item["signal_id"] for item in diagnostics
    }


def test_proof_queue_diagnoses_source_extension_build_plan_missing(
    tmp_path: Path, capsys: pytest.CaptureFixture[str]
) -> None:
    db = tmp_path / "proof_queue.sqlite3"
    log_path = tmp_path / "source-extension-plan.log"
    conn = state._connect(db)
    scheduling._insert_run(
        conn,
        run_id="source-extension-plan",
        logical_id="e1-numpy-multiarray-rebuild",
        reason="prove source-extension source-plan path diagnosis",
        command=[sys.executable, "-m", "molt", "extension", "build"],
        cwd=state.ROOT,
        resource_family="wasm-source-extension",
        contention_key="wasm:pact-seal-regen",
        scopes=["src/molt/cli/source_extensions.py"],
        git_snapshot={
            "available": True,
            "head": "abc123",
            "dirty": False,
            "status": [],
        },
        log_path=log_path,
        summary_json=tmp_path / "source-extension-plan.memory_guard.json",
    )
    log_path.write_text(
        '{"schema_version": "1.0", "command": "extension-build", '
        '"status": "error", "errors": ["Extension build configuration errors: '
        "source extension build plan not found: "
        'C:\\repo\\numpy\\tmp\\pact_numpy_multiarray\\intro-targets.json"]}\n',
        encoding="utf-8",
    )
    state._update_run(conn, "source-extension-plan", status="failed", returncode=2)

    assert (
        cli.main(
            [
                "--db",
                str(db),
                "--logs-root",
                str(tmp_path / "runs"),
                "--repo-root",
                str(state.ROOT),
                "evidence",
                "--run-id",
                "source-extension-plan",
            ]
        )
        == 0
    )
    evidence = json.loads(capsys.readouterr().out)
    diagnostics = evidence[0]["diagnostics"]
    assert diagnostics[0]["signal_id"] == "source-extension-build-plan-missing"
    assert diagnostics[0]["severity"] == "infra"
    assert "intro-targets.json" in diagnostics[0]["summary"]
    assert "source-extension package custody" in diagnostics[0]["next_action"]
    assert "do not hand-author package metadata" in diagnostics[0]["next_action"]
    assert str(log_path) in diagnostics[0]["artifacts"]
    assert "unclassified-failed-proof" not in {
        item["signal_id"] for item in diagnostics
    }


def test_proof_queue_diagnoses_source_extension_compile_header_missing(
    tmp_path: Path, capsys: pytest.CaptureFixture[str]
) -> None:
    db = tmp_path / "proof_queue.sqlite3"
    log_path = tmp_path / "source-extension-header.log"
    conn = state._connect(db)
    scheduling._insert_run(
        conn,
        run_id="source-extension-header",
        logical_id="e1-scipy-ndimage-rebuild",
        reason="prove source-extension missing generated header diagnosis",
        command=[sys.executable, "-m", "molt", "extension", "build"],
        cwd=state.ROOT,
        resource_family="wasm-source-extension",
        contention_key="wasm:pact-seal-regen",
        scopes=["src/molt/cli/source_extensions.py"],
        git_snapshot={
            "available": True,
            "head": "abc123",
            "dirty": False,
            "status": [],
        },
        log_path=log_path,
        summary_json=tmp_path / "source-extension-header.memory_guard.json",
    )
    log_path.write_text(
        '{"schema_version": "1.0", "command": "extension-build", '
        '"status": "error", "errors": ["Failed compiling nd_image.c: '
        "In file included from C:\\repo\\scipy\\ndimage\\src\\nd_image.c:45:\\n"
        "C:\\repo\\scipy\\_lib\\src\\ccallback.h:25:10: fatal error: "
        "'scipy_config.h' file not found\\n"
        '1 error generated."]}\n',
        encoding="utf-8",
    )
    state._update_run(conn, "source-extension-header", status="failed", returncode=2)

    assert (
        cli.main(
            [
                "--db",
                str(db),
                "--logs-root",
                str(tmp_path / "runs"),
                "--repo-root",
                str(state.ROOT),
                "evidence",
                "--run-id",
                "source-extension-header",
            ]
        )
        == 0
    )
    evidence = json.loads(capsys.readouterr().out)
    diagnostics = evidence[0]["diagnostics"]
    assert diagnostics[0]["signal_id"] == "source-extension-compile-header-missing"
    assert diagnostics[0]["severity"] == "infra"
    assert "scipy_config.h" in diagnostics[0]["summary"]
    assert "package build metadata" in diagnostics[0]["next_action"]
    assert "do not copy headers" in diagnostics[0]["next_action"]
    assert str(log_path) in diagnostics[0]["artifacts"]
    assert "unclassified-failed-proof" not in {
        item["signal_id"] for item in diagnostics
    }


def test_proof_queue_diagnoses_missing_locked_source_build_environment(
    tmp_path: Path, capsys: pytest.CaptureFixture[str]
) -> None:
    db = tmp_path / "proof_queue.sqlite3"
    log_path = tmp_path / "source-build-environment.log"
    conn = state._connect(db)
    scheduling._insert_run(
        conn,
        run_id="source-build-environment",
        logical_id="pact-numpy-canonical-seal",
        reason="prove locked source-build environment diagnosis",
        command=[sys.executable, "-m", "molt", "extension", "produce-set"],
        cwd=state.ROOT,
        resource_family="wasm-source-extension",
        contention_key="wasm:pact-seal-regen",
        scopes=["src/molt/cli/source_build_environment.py"],
        git_snapshot={
            "available": True,
            "head": "abc123",
            "dirty": False,
            "status": [],
        },
        log_path=log_path,
        summary_json=tmp_path / "source-build-environment.memory_guard.json",
    )
    log_path.write_text(
        "source build environment is not pre-provisioned from locked custody; "
        "the producer never mutates its active interpreter. Missing or "
        "out-of-range requirements: meson-python>=0.18.0, Cython>=3.0.6\n",
        encoding="utf-8",
    )
    state._update_run(conn, "source-build-environment", status="failed", returncode=2)

    assert (
        cli.main(
            [
                "--db",
                str(db),
                "--logs-root",
                str(tmp_path / "runs"),
                "--repo-root",
                str(state.ROOT),
                "evidence",
                "--run-id",
                "source-build-environment",
            ]
        )
        == 0
    )
    evidence = json.loads(capsys.readouterr().out)
    diagnostics = evidence[0]["diagnostics"]
    assert diagnostics[0]["signal_id"] == ("source-build-environment-custody-missing")
    assert diagnostics[0]["severity"] == "infra"
    assert "meson-python>=0.18.0" in diagnostics[0]["summary"]
    assert "configured dependency group" in diagnostics[0]["next_action"]
    assert "ambient project interpreter" in diagnostics[0]["next_action"]
    assert "unclassified-failed-proof" not in {
        item["signal_id"] for item in diagnostics
    }


def test_proof_queue_diagnoses_locked_console_script_path_custody(
    tmp_path: Path, capsys: pytest.CaptureFixture[str]
) -> None:
    db = tmp_path / "proof_queue.sqlite3"
    log_path = tmp_path / "source-build-console-path.log"
    summary_path = tmp_path / "source-build-console-path.memory_guard.json"
    conn = state._connect(db)
    scheduling._insert_run(
        conn,
        run_id="source-build-console-path",
        logical_id="pact-numpy-canonical-seal",
        reason="prove locked console-script PATH custody diagnosis",
        command=[sys.executable, "-m", "molt", "extension", "produce-set"],
        cwd=state.ROOT,
        resource_family="wasm-source-extension",
        contention_key="wasm:pact-seal-regen",
        scopes=["src/molt/cli/source_extension_producer.py"],
        git_snapshot={
            "available": True,
            "head": "abc123",
            "dirty": False,
            "status": [],
        },
        log_path=log_path,
        summary_json=summary_path,
    )
    log_path.write_text(
        "meson.build:1:0: ERROR: Unknown compiler(s): "
        "[['cython'], ['cython3']]\n"
        'Running `cython -V` gave "[WinError 2] The system cannot find the '
        'file specified"\n'
        'Running `cython3 -V` gave "[WinError 2] The system cannot find the '
        'file specified"\n',
        encoding="utf-8",
    )
    state._update_run(conn, "source-build-console-path", status="failed", returncode=2)

    assert (
        cli.main(
            [
                "--db",
                str(db),
                "--logs-root",
                str(tmp_path / "runs"),
                "--repo-root",
                str(state.ROOT),
                "evidence",
                "--run-id",
                "source-build-console-path",
            ]
        )
        == 0
    )
    evidence = json.loads(capsys.readouterr().out)
    diagnostics = evidence[0]["diagnostics"]
    assert [item["signal_id"] for item in diagnostics] == [
        "source-build-console-script-path-custody"
    ]
    assert diagnostics[0]["severity"] == "infra"
    assert "Cython console script" in diagnostics[0]["summary"]
    assert "Scripts/bin directory first" in diagnostics[0]["next_action"]
    assert (
        "Never install Cython into an ambient interpreter"
        in diagnostics[0]["next_action"]
    )
    assert "pin an older version" in diagnostics[0]["next_action"]
    assert diagnostics[0]["artifacts"] == [str(summary_path), str(log_path)]


@pytest.mark.parametrize(
    "near_miss",
    [
        "Unknown compiler(s): [['cython'], ['cython3']]\n"
        "Running `cython -V` gave Cython 2.7 is out of range\n",
        "Unknown compiler(s): [['fortran']]\n"
        'Running `gfortran --version` gave "[WinError 2] file not found"\n',
        'Running `cython -V` gave "[WinError 2] file not found"\n',
    ],
)
def test_locked_console_script_path_diagnostic_rejects_near_misses(
    near_miss: str,
) -> None:
    assert (
        diagnostics_module.SOURCE_BUILD_CONSOLE_SCRIPT_PATH_CUSTODY_RE.search(near_miss)
        is None
    )


def test_proof_queue_diagnoses_source_extension_cython_regeneration_failed(
    tmp_path: Path, capsys: pytest.CaptureFixture[str]
) -> None:
    db = tmp_path / "proof_queue.sqlite3"
    log_path = tmp_path / "source-extension-cython.log"
    conn = state._connect(db)
    scheduling._insert_run(
        conn,
        run_id="source-extension-cython",
        logical_id="e1-scipy-ni-label-rebuild",
        reason="prove source-extension Cython regeneration diagnosis",
        command=[sys.executable, "-m", "molt", "extension", "build"],
        cwd=state.ROOT,
        resource_family="wasm-source-extension",
        contention_key="wasm:pact-seal-regen",
        scopes=["src/molt/cli/source_extensions.py"],
        git_snapshot={
            "available": True,
            "head": "abc123",
            "dirty": False,
            "status": [],
        },
        log_path=log_path,
        summary_json=tmp_path / "source-extension-cython.memory_guard.json",
    )
    log_path.write_text(
        '{"schema_version": "1.0", "command": "extension-build", '
        '"status": "error", "errors": ["Standalone `cython -3` regeneration '
        "of _ni_label.pyx failed: AttributeError: 'NoneType' object has no "
        "attribute 'is_builtin_type'\"]}\n",
        encoding="utf-8",
    )
    state._update_run(conn, "source-extension-cython", status="failed", returncode=2)

    assert (
        cli.main(
            [
                "--db",
                str(db),
                "--logs-root",
                str(tmp_path / "runs"),
                "--repo-root",
                str(state.ROOT),
                "evidence",
                "--run-id",
                "source-extension-cython",
            ]
        )
        == 0
    )
    evidence = json.loads(capsys.readouterr().out)
    diagnostics = evidence[0]["diagnostics"]
    assert diagnostics[0]["signal_id"] == "source-extension-cython-regeneration-failed"
    assert diagnostics[0]["severity"] == "infra"
    assert "_ni_label.pyx" in diagnostics[0]["summary"]
    assert "package's declared build metadata" in diagnostics[0]["next_action"]
    assert (
        "do not add a package-specific standalone Cython command"
        in diagnostics[0]["next_action"]
    )
    assert str(log_path) in diagnostics[0]["artifacts"]
    assert "unclassified-failed-proof" not in {
        item["signal_id"] for item in diagnostics
    }


def test_proof_queue_diagnoses_source_extension_cimport_header_mismatch(
    tmp_path: Path, capsys: pytest.CaptureFixture[str]
) -> None:
    db = tmp_path / "proof_queue.sqlite3"
    log_path = tmp_path / "source-extension-cimport-header.log"
    conn = state._connect(db)
    scheduling._insert_run(
        conn,
        run_id="source-extension-cimport-header",
        logical_id="e1-scipy-ni-label-rebuild",
        reason="prove source-extension cimport/header custody diagnosis",
        command=[sys.executable, "-m", "molt", "extension", "build"],
        cwd=state.ROOT,
        resource_family="wasm-source-extension",
        contention_key="wasm:pact-seal-regen",
        scopes=["src/molt/cli/source_extension_cython.py"],
        git_snapshot={
            "available": True,
            "head": "abc123",
            "dirty": False,
            "status": [],
        },
        log_path=log_path,
        summary_json=tmp_path / "source-extension-cimport-header.memory_guard.json",
    )
    log_path.write_text(
        '{"schema_version": "1.0", "command": "extension-build", '
        '"status": "error", "errors": ["Failed compiling _ni_label.c: '
        "cython_standalone/_ni_label.c:18976:13: error: call to undeclared "
        "function 'PyDataType_TYPEOBJ'; ISO C99 and later do not support "
        "implicit function declarations [-Wimplicit-function-declaration]\\n"
        "cython_standalone/_ni_label.c:20142:13: error: call to undeclared "
        "function '_PyUFuncObject_GET_ITEM_DATA'; ISO C99 and later do not "
        'support implicit function declarations"]}\n',
        encoding="utf-8",
    )
    state._update_run(
        conn, "source-extension-cimport-header", status="failed", returncode=2
    )

    assert (
        cli.main(
            [
                "--db",
                str(db),
                "--logs-root",
                str(tmp_path / "runs"),
                "--repo-root",
                str(state.ROOT),
                "evidence",
                "--run-id",
                "source-extension-cimport-header",
            ]
        )
        == 0
    )
    evidence = json.loads(capsys.readouterr().out)
    diagnostics = evidence[0]["diagnostics"]
    assert diagnostics[0]["signal_id"] == "source-extension-cimport-header-mismatch"
    assert diagnostics[0]["severity"] == "infra"
    assert "PyDataType_TYPEOBJ" in diagnostics[0]["summary"]
    assert "same build-interpreter package custody" in diagnostics[0]["next_action"]
    assert "do not pin an older Cython" in diagnostics[0]["next_action"]
    assert str(log_path) in diagnostics[0]["artifacts"]
    assert "unclassified-failed-proof" not in {
        item["signal_id"] for item in diagnostics
    }


def test_proof_queue_diagnoses_source_extension_cpython_abi_declaration_missing(
    tmp_path: Path, capsys: pytest.CaptureFixture[str]
) -> None:
    db = tmp_path / "proof_queue.sqlite3"
    log_path = tmp_path / "source-extension-cpython-abi-decl.log"
    conn = state._connect(db)
    scheduling._insert_run(
        conn,
        run_id="source-extension-cpython-abi-decl",
        logical_id="e1-scipy-ni-label-rebuild",
        reason="prove source-extension cpython abi declaration diagnosis",
        command=[sys.executable, "-m", "molt", "extension", "build"],
        cwd=state.ROOT,
        resource_family="wasm-source-extension",
        contention_key="wasm:pact-seal-regen",
        scopes=["src/molt/cli/source_extension_cython.py"],
        git_snapshot={
            "available": True,
            "head": "abc123",
            "dirty": False,
            "status": [],
        },
        log_path=log_path,
        summary_json=tmp_path / "source-extension-cpython-abi-decl.memory_guard.json",
    )
    log_path.write_text(
        '{"schema_version": "1.0", "command": "extension-build", '
        '"status": "error", "errors": ["Failed compiling _ni_label.c: '
        "cython_standalone/_ni_label.c:31134:73: error: call to undeclared "
        "function 'PyType_IS_GC'; ISO C99 and later do not support implicit "
        "function declarations [-Wimplicit-function-declaration]\\n"
        "runtime\\\\molt-cpython-abi\\\\include\\\\Python.h:1031:21: note: "
        "'PyTraceBack_Here' declared here\"]}\n",
        encoding="utf-8",
    )
    state._update_run(
        conn, "source-extension-cpython-abi-decl", status="failed", returncode=2
    )

    assert (
        cli.main(
            [
                "--db",
                str(db),
                "--logs-root",
                str(tmp_path / "runs"),
                "--repo-root",
                str(state.ROOT),
                "evidence",
                "--run-id",
                "source-extension-cpython-abi-decl",
            ]
        )
        == 0
    )
    evidence = json.loads(capsys.readouterr().out)
    diagnostics = evidence[0]["diagnostics"]
    assert (
        diagnostics[0]["signal_id"]
        == "source-extension-cpython-abi-declaration-missing"
    )
    assert diagnostics[0]["severity"] == "error"
    assert "PyType_IS_GC" in diagnostics[0]["summary"]
    assert "cpython-abi owner" in diagnostics[0]["next_action"]
    assert "Do not relax compiler diagnostics" in diagnostics[0]["next_action"]
    assert str(log_path) in diagnostics[0]["artifacts"]
    assert "unclassified-failed-proof" not in {
        item["signal_id"] for item in diagnostics
    }


def test_proof_queue_diagnoses_cpython_abi_pymod_gil_slot_token_mismatch(
    tmp_path: Path, capsys: pytest.CaptureFixture[str]
) -> None:
    db = tmp_path / "proof_queue.sqlite3"
    log_path = tmp_path / "pymod-gil-slot.log"
    conn = state._connect(db)
    scheduling._insert_run(
        conn,
        run_id="pymod-gil-slot",
        logical_id="e1-scipy-ndimage-rebuild",
        reason="prove cpython abi PyModuleDef slot token diagnosis",
        command=[sys.executable, "-m", "molt", "extension", "build"],
        cwd=state.ROOT,
        resource_family="wasm-source-extension",
        contention_key="wasm:pact-seal-regen",
        scopes=["runtime/molt-cpython-abi/include/Python.h"],
        git_snapshot={
            "available": True,
            "head": "abc123",
            "dirty": False,
            "status": [],
        },
        log_path=log_path,
        summary_json=tmp_path / "pymod-gil-slot.memory_guard.json",
    )
    log_path.write_text(
        '{"schema_version": "1.0", "command": "extension-build", '
        '"status": "error", "errors": ["Failed compiling nd_image.c: '
        "nd_image.c:1364:36: error: incompatible integer to pointer conversion "
        "initializing 'void *' with an expression of type 'int' "
        "[-Wint-conversion]\\n"
        " 1364 |     {Py_mod_multiple_interpreters, "
        "Py_MOD_PER_INTERPRETER_GIL_SUPPORTED},\\n"
        "      |                                    "
        "^~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~\\n"
        "Python.h:760:46: note: expanded from macro "
        "'Py_MOD_PER_INTERPRETER_GIL_SUPPORTED'\\n"
        "  760 | #define Py_MOD_PER_INTERPRETER_GIL_SUPPORTED 2\\n"
        '      |                                              ^"]}\n',
        encoding="utf-8",
    )
    state._update_run(conn, "pymod-gil-slot", status="failed", returncode=2)

    assert (
        cli.main(
            [
                "--db",
                str(db),
                "--logs-root",
                str(tmp_path / "runs"),
                "--repo-root",
                str(state.ROOT),
                "evidence",
                "--run-id",
                "pymod-gil-slot",
            ]
        )
        == 0
    )
    evidence = json.loads(capsys.readouterr().out)
    diagnostics = evidence[0]["diagnostics"]
    assert diagnostics[0]["signal_id"] == "cpython-abi-pymod-gil-slot-token-mismatch"
    assert diagnostics[0]["severity"] == "error"
    assert "Py_MOD_PER_INTERPRETER_GIL_SUPPORTED" in diagnostics[0]["summary"]
    assert "cpython-abi owner" in diagnostics[0]["next_action"]
    assert "reusable C-API primitive" in diagnostics[0]["next_action"]
    assert str(log_path) in diagnostics[0]["artifacts"]
    assert "unclassified-failed-proof" not in {
        item["signal_id"] for item in diagnostics
    }


def test_proof_queue_diagnoses_source_lease_contamination(
    tmp_path: Path, capsys: pytest.CaptureFixture[str]
) -> None:
    db = tmp_path / "proof_queue.sqlite3"
    log_path = tmp_path / "source-lease.log"
    conn = state._connect(db)
    scheduling._insert_run(
        conn,
        run_id="source-lease-run",
        logical_id="source-lease",
        reason="prove source lease contamination diagnosis",
        command=[sys.executable, "tests/molt_diff.py", "case.py"],
        cwd=state.ROOT,
        resource_family="python",
        contention_key="python:source-lease",
        scopes=["src/molt/stdlib/sys.py"],
        git_snapshot={
            "available": True,
            "head": "abc123",
            "dirty": True,
            "status": [" M src/molt/stdlib/sys.py"],
        },
        log_path=log_path,
        summary_json=tmp_path / "source-lease.memory_guard.json",
    )
    log_path.write_text(
        r"Failed to read module C:\repo\src\molt\stdlib\sys.py: "
        r"Source lease for C:\repo\src\molt\stdlib\sys.py "
        "changed size during compile\n"
        "proof_queue finished status=failed exit_code=1 elapsed=17.0s\n",
        encoding="utf-8",
    )
    state._update_run(conn, "source-lease-run", status="failed", returncode=1)

    assert (
        cli.main(
            [
                "--db",
                str(db),
                "--logs-root",
                str(tmp_path / "runs"),
                "--repo-root",
                str(state.ROOT),
                "evidence",
                "--run-id",
                "source-lease-run",
            ]
        )
        == 0
    )
    evidence = json.loads(capsys.readouterr().out)
    diagnostics = evidence[0]["diagnostics"]
    assert diagnostics[0]["signal_id"] == "source-lease-changed-during-proof"
    assert diagnostics[0]["severity"] == "operator"
    assert "contaminated evidence" in diagnostics[0]["summary"]
    assert "src\\molt\\stdlib\\sys.py" in diagnostics[0]["evidence"]
    assert "unclassified-failed-proof" not in {
        item["signal_id"] for item in diagnostics
    }


def test_proof_queue_diagnoses_partial_module_publication(
    tmp_path: Path, capsys: pytest.CaptureFixture[str]
) -> None:
    db = tmp_path / "proof_queue.sqlite3"
    log_path = tmp_path / "partial-module-publication.log"
    conn = state._connect(db)
    scheduling._insert_run(
        conn,
        run_id="partial-module-publication-run",
        logical_id="partial-module-publication",
        reason="prove partial module publication diagnosis",
        command=[sys.executable, "tests/molt_diff.py", "case.py"],
        cwd=state.ROOT,
        resource_family="python",
        contention_key="python:partial-module-publication",
        scopes=["runtime/molt-runtime/src/builtins/module_table.rs"],
        git_snapshot={
            "available": True,
            "head": "abc123",
            "dirty": False,
            "status": [],
        },
        log_path=log_path,
        summary_json=tmp_path / "partial-module-publication.memory_guard.json",
    )
    log_path.write_text(
        "[RUN] tests\\differential\\stdlib\\queue_shutdown_version_gate.py\n"
        "[FAIL] tests\\differential\\stdlib\\queue_shutdown_version_gate.py "
        "(native) mismatch: stdout mismatch; exit code ref=0 cand=1\n"
        '  Molt    return: 1 stderr: "Traceback (most recent call last):\\n'
        "ImportError: cannot import partially initialized module "
        "'importlib.machinery' before its publication "
        '(circular import during module allocation)\\n"\n',
        encoding="utf-8",
    )
    state._update_run(
        conn, "partial-module-publication-run", status="failed", returncode=1
    )

    assert (
        cli.main(
            [
                "--db",
                str(db),
                "--logs-root",
                str(tmp_path / "runs"),
                "--repo-root",
                str(state.ROOT),
                "evidence",
                "--run-id",
                "partial-module-publication-run",
            ]
        )
        == 0
    )
    evidence = json.loads(capsys.readouterr().out)
    diagnostics = evidence[0]["diagnostics"]
    assert diagnostics[0]["signal_id"] == "import-partial-module-publication"
    assert diagnostics[0]["severity"] == "error"
    assert "importlib.machinery" in diagnostics[0]["summary"]
    assert (
        "tests\\differential\\stdlib\\queue_shutdown_version_gate.py"
        in diagnostics[0]["summary"]
    )
    assert "[FAIL] tests\\differential\\stdlib\\queue_shutdown_version_gate.py" in str(
        diagnostics[0]["evidence"]
    )
    assert (
        "tests\\differential\\stdlib\\queue_shutdown_version_gate.py"
        in diagnostics[0]["scopes"]
    )
    assert (
        "runtime/molt-runtime/src/builtins/module_table.rs" in diagnostics[0]["scopes"]
    )
    assert "unclassified-failed-proof" not in {
        item["signal_id"] for item in diagnostics
    }


def test_proof_queue_diagnoses_molt_diff_stdout_mismatch(
    tmp_path: Path, capsys: pytest.CaptureFixture[str]
) -> None:
    db = tmp_path / "proof_queue.sqlite3"
    log_path = tmp_path / "molt-diff-stdout.log"
    conn = state._connect(db)
    scheduling._insert_run(
        conn,
        run_id="molt-diff-stdout-run",
        logical_id="molt-diff-stdout",
        reason="prove molt_diff stdout mismatch diagnosis",
        command=[
            sys.executable,
            "tests/molt_diff.py",
            "--jobs",
            "1",
            "tests/differential/stdlib/sys_encoding_basic.py",
        ],
        cwd=state.ROOT,
        resource_family="python",
        contention_key="python:r6-sys-stat-py312",
        scopes=[
            "tests/molt_diff.py",
            "tests/differential/stdlib/sys_encoding_basic.py",
        ],
        git_snapshot={
            "available": True,
            "head": "abc123",
            "dirty": False,
            "status": [],
        },
        log_path=log_path,
        summary_json=tmp_path / "molt-diff-stdout.memory_guard.json",
    )
    log_path.write_text(
        "[RUN] tests\\differential\\stdlib\\sys_encoding_basic.py\n"
        "Testing tests\\differential\\stdlib\\sys_encoding_basic.py against python...\n"
        "[FAIL] tests\\differential\\stdlib\\sys_encoding_basic.py "
        "(native) mismatch: stdout mismatch\n"
        "  CPython stdout: 'utf-8\\nutf-8\\nsurrogatepass\\nTrue\\nTrue\\n'\n"
        "  Molt    stdout: 'utf-8\\nutf-8\\nsurrogateescape\\nTrue\\nTrue\\n'\n"
        "  CPython return: 0 stderr: ''\n"
        "  Molt    return: 0 stderr: ''\n"
        "proof_queue finished status=failed exit_code=1 elapsed=365.719s\n",
        encoding="utf-8",
    )
    state._update_run(conn, "molt-diff-stdout-run", status="failed", returncode=1)

    assert (
        cli.main(
            [
                "--db",
                str(db),
                "--logs-root",
                str(tmp_path / "runs"),
                "--repo-root",
                str(state.ROOT),
                "evidence",
                "--run-id",
                "molt-diff-stdout-run",
            ]
        )
        == 0
    )
    evidence = json.loads(capsys.readouterr().out)
    diagnostics = evidence[0]["diagnostics"]
    assert diagnostics[0]["signal_id"] == "molt-diff-output-mismatch"
    assert diagnostics[0]["severity"] == "error"
    assert "sys_encoding_basic.py" in diagnostics[0]["summary"]
    assert "stdout mismatch" in diagnostics[0]["summary"]
    assert "surrogatepass" in diagnostics[0]["evidence"]
    assert "surrogateescape" in diagnostics[0]["evidence"]
    assert "tests/differential/stdlib/sys_encoding_basic.py" in diagnostics[0]["scopes"]
    assert "unclassified-failed-proof" not in {
        item["signal_id"] for item in diagnostics
    }


def test_proof_queue_diagnoses_pytest_import_error(
    tmp_path: Path, capsys: pytest.CaptureFixture[str]
) -> None:
    db = tmp_path / "proof_queue.sqlite3"
    log_path = tmp_path / "pytest-import-error.log"
    conn = state._connect(db)
    scheduling._insert_run(
        conn,
        run_id="pytest-import-error-run",
        logical_id="pytest-import-error",
        reason="prove pytest import diagnostics",
        command=[sys.executable, "-m", "pytest", "tests/test_molt_dev.py"],
        cwd=state.ROOT,
        resource_family="python",
        contention_key="python:pytest-import-error",
        scopes=["tests/test_molt_dev.py"],
        git_snapshot={
            "available": True,
            "head": "abc123",
            "dirty": False,
            "status": [],
        },
        log_path=log_path,
        summary_json=tmp_path / "pytest-import-error.memory_guard.json",
    )
    state._insert_note(
        conn,
        run_id="pytest-import-error-run",
        body="test: capture pytest import diagnostics",
        kind="submission",
        author="codex",
    )
    log_path.write_text(
        "\n".join(
            [
                "E   ImportError: cannot import name '_run_fast_captured_command' from 'molt_dev_common'",
                "ERROR tests/test_molt_dev.py::test_secure_wip_honors_ignore_set - ImportError: cannot import name '_run_fast_captured_command'",
                "1 error in 0.42s",
            ]
        ),
        encoding="utf-8",
    )
    state._update_run(conn, "pytest-import-error-run", status="failed", returncode=1)

    assert (
        cli.main(
            [
                "--db",
                str(db),
                "--logs-root",
                str(tmp_path / "runs"),
                "--repo-root",
                str(state.ROOT),
                "evidence",
                "--run-id",
                "pytest-import-error-run",
            ]
        )
        == 0
    )
    evidence = json.loads(capsys.readouterr().out)
    diagnostics = evidence[0]["diagnostics"]
    assert diagnostics[0]["signal_id"] == "pytest-error"
    assert "test_secure_wip_honors_ignore_set" in str(diagnostics[0]["summary"])
    assert "_run_fast_captured_command" in str(diagnostics[0]["evidence"])


def test_proof_queue_diagnoses_external_native_and_profile_refusals(
    tmp_path: Path, capsys: pytest.CaptureFixture[str]
) -> None:
    db = tmp_path / "proof_queue.sqlite3"
    conn = state._connect(db)
    cases = [
        (
            "native-artifact",
            "External static package native-artifact custody errors: "
            "scipy: callable export 'scipy.ndimage.distance_transform_edt' uses "
            "module_attr provider 'scipy.ndimage._nd_image', but "
            "'distance_transform_edt' is not declared by a PyMethodDef entry in "
            "the admitted extension sources.",
            "external-native-artifact-custody",
        ),
        (
            "native-support",
            "reachable native support source imports native package modules without "
            "source or artifact custody: scipy._external.packaging_version "
            "(no .pyx/.c/.cpp source candidate found under the admitted package roots).",
            "external-native-support-custody",
        ),
        (
            "profile-refusal",
            "Profile 'micro' excludes the 'stdlib_regex' runtime feature that this "
            "program's REACHED code requires.",
            "stdlib-profile-refusal",
        ),
    ]
    for run_id, log_text, _signal_id in cases:
        log_path = tmp_path / f"{run_id}.log"
        scheduling._insert_run(
            conn,
            run_id=run_id,
            logical_id="pact-witness-acceptance",
            reason="prove external native diagnostics",
            command=[sys.executable, "-c", "raise SystemExit(2)"],
            cwd=state.ROOT,
            resource_family="wasm-browser",
            contention_key=f"wasm:{run_id}",
            scopes=["src/molt/cli/external_native.py"],
            git_snapshot={
                "available": True,
                "head": "abc123",
                "dirty": False,
                "status": [],
            },
            log_path=log_path,
            summary_json=tmp_path / f"{run_id}.memory_guard.json",
        )
        state._insert_note(
            conn,
            run_id=run_id,
            body="test: classify recurring Pact build refusal",
            kind="submission",
            author="codex",
        )
        log_path.write_text(log_text + "\n", encoding="utf-8")
        state._update_run(conn, run_id, status="failed", returncode=2)

    assert (
        cli.main(
            [
                "--db",
                str(db),
                "--logs-root",
                str(tmp_path / "runs"),
                "--repo-root",
                str(state.ROOT),
                "evidence",
                "--limit",
                "3",
            ]
        )
        == 0
    )
    evidence = json.loads(capsys.readouterr().out)
    signal_ids = {
        item["diagnostics"][0]["signal_id"] for item in evidence if item["diagnostics"]
    }
    assert signal_ids == {
        "external-native-artifact-custody",
        "external-native-support-custody",
        "stdlib-profile-refusal",
    }


def test_proof_queue_diagnoses_external_native_runtime_import_custody(
    tmp_path: Path, capsys: pytest.CaptureFixture[str]
) -> None:
    db = tmp_path / "proof_queue.sqlite3"
    log_path = tmp_path / "runtime-import-custody.log"
    conn = state._connect(db)
    scheduling._insert_run(
        conn,
        run_id="runtime-import-custody",
        logical_id="pact-witness-acceptance",
        reason="prove stale NumPy seal runtime import custody diagnostics",
        command=[sys.executable, "-c", "raise SystemExit(2)"],
        cwd=state.ROOT,
        resource_family="wasm-browser",
        contention_key="wasm:pact-witness",
        scopes=["src/molt/cli/external_native.py"],
        git_snapshot={
            "available": True,
            "head": "abc123",
            "dirty": False,
            "status": [],
        },
        log_path=log_path,
        summary_json=tmp_path / "runtime-import-custody.memory_guard.json",
    )
    log_path.write_text(
        "External static package native-artifact custody errors: "
        "numpy: sealed extension manifest lacks a "
        "'runtime_python_import_modules' field and its C sources no longer "
        "resolve, so its runtime Python import closure cannot be proven. "
        "Re-seal the extension root through 'molt extension seal' to persist "
        "the source-derived runtime imports. Unresolved sources: "
        "object_closure.objects[0].source: "
        "tmp/worktrees/pact-collab/deleted/npy_static_data.c does not exist\n",
        encoding="utf-8",
    )
    state._update_run(
        conn,
        "runtime-import-custody",
        status="failed",
        returncode=2,
    )

    assert (
        cli.main(
            [
                "--db",
                str(db),
                "--logs-root",
                str(tmp_path / "runs"),
                "--repo-root",
                str(state.ROOT),
                "evidence",
                "--run-id",
                "runtime-import-custody",
            ]
        )
        == 0
    )
    evidence = json.loads(capsys.readouterr().out)
    diagnostics = evidence[0]["diagnostics"]
    assert [item["signal_id"] for item in diagnostics] == [
        "external-native-runtime-import-custody"
    ]
    diagnostic = diagnostics[0]
    assert "numpy" in diagnostic["summary"]
    assert "runtime_python_import_modules" in diagnostic["summary"]
    assert "object_closure.objects[0].source" in diagnostic["evidence"]
    assert "pact-witness-acceptance" in diagnostic["next_action"]


def test_proof_queue_diagnoses_external_native_abi_link_surface_gap(
    tmp_path: Path, capsys: pytest.CaptureFixture[str]
) -> None:
    db = tmp_path / "proof_queue.sqlite3"
    log_path = tmp_path / "abi-link-surface.log"
    conn = state._connect(db)
    scheduling._insert_run(
        conn,
        run_id="abi-link-surface",
        logical_id="pact-witness-acceptance",
        reason="prove generated ABI link surface diagnostics",
        command=[sys.executable, "-c", "raise SystemExit(2)"],
        cwd=state.ROOT,
        resource_family="wasm-browser",
        contention_key="wasm:pact-witness",
        scopes=["src/molt/cli/external_native.py"],
        git_snapshot={
            "available": True,
            "head": "abc123",
            "dirty": False,
            "status": [],
        },
        log_path=log_path,
        summary_json=tmp_path / "abi-link-surface.memory_guard.json",
    )
    state._insert_note(
        conn,
        run_id="abi-link-surface",
        body="test: classify generated WASM ABI link import surface gaps",
        kind="submission",
        author="codex",
    )
    log_path.write_text(
        "External static package native-artifact custody errors: "
        "numpy: object_closure runtime ABI symbol "
        "'molt_cpython_abi_date_from_date' is not in the generated WASM "
        "ABI/link import surface; numpy: object_closure runtime ABI symbol "
        "'molt_cpython_abi_delta_from_delta' is not in the generated WASM "
        "ABI/link import surface\n",
        encoding="utf-8",
    )
    state._update_run(conn, "abi-link-surface", status="failed", returncode=2)

    assert (
        cli.main(
            [
                "--db",
                str(db),
                "--logs-root",
                str(tmp_path / "runs"),
                "--repo-root",
                str(state.ROOT),
                "evidence",
                "--run-id",
                "abi-link-surface",
            ]
        )
        == 0
    )
    evidence = json.loads(capsys.readouterr().out)
    diagnostics = evidence[0]["diagnostics"]
    assert [item["signal_id"] for item in diagnostics] == [
        "external-native-abi-link-surface-missing"
    ]
    assert "molt_cpython_abi_date_from_date" in diagnostics[0]["summary"]
    assert "generated WASM ABI manifest" in diagnostics[0]["next_action"]


def test_proof_queue_audit_distinguishes_classified_product_failure(
    tmp_path: Path, capsys: pytest.CaptureFixture[str]
) -> None:
    db = tmp_path / "proof_queue.sqlite3"
    log_path = tmp_path / "classified.log"
    conn = state._connect(db)
    scheduling._insert_run(
        conn,
        run_id="classified-run",
        logical_id="pact-witness-acceptance",
        reason="prove classified product failure is not queue debt",
        command=[sys.executable, "-c", "raise SystemExit(1)"],
        cwd=state.ROOT,
        resource_family="wasm",
        contention_key="wasm:pact-witness",
        scopes=["collab/pact/"],
        git_snapshot={
            "available": True,
            "head": "abc123",
            "dirty": False,
            "status": [],
        },
        log_path=log_path,
        summary_json=tmp_path / "classified.memory_guard.json",
    )
    state._insert_note(
        conn,
        run_id="classified-run",
        body="finding: product failure is classified",
        kind="finding",
        author="codex",
    )
    log_path.write_text(
        "ImportError: _nd_image: static-link PyModuleDef Py_mod_exec slot returned non-zero\n",
        encoding="utf-8",
    )
    state._update_run(conn, "classified-run", status="failed", returncode=1)

    assert (
        cli.main(
            [
                "--db",
                str(db),
                "--logs-root",
                str(tmp_path / "runs"),
                "--repo-root",
                str(state.ROOT),
                "audit",
                "--no-notebook-check",
            ]
        )
        == 0
    )
    output = capsys.readouterr().out
    assert "classified_failed=1" in output
    assert "no queue health issues" in output


def test_proof_queue_audit_flags_weak_metadata(
    tmp_path: Path, capsys: pytest.CaptureFixture[str]
) -> None:
    db = tmp_path / "proof_queue.sqlite3"
    log_path = tmp_path / "weak-metadata.log"
    conn = state._connect(db)
    scheduling._insert_run(
        conn,
        run_id="weak-metadata-run",
        logical_id="bad-shell-quote",
        reason='"Prove',
        command=[sys.executable, "-c", "print('passed with weak metadata')"],
        cwd=state.ROOT,
        resource_family="generic",
        contention_key="generic:default",
        scopes=[],
        git_snapshot={
            "available": True,
            "head": "abc123",
            "dirty": False,
            "status": [],
        },
        log_path=log_path,
        summary_json=tmp_path / "weak-metadata.memory_guard.json",
    )
    log_path.write_text("passed with weak metadata\n", encoding="utf-8")
    state._update_run(
        conn,
        "weak-metadata-run",
        status="passed",
        returncode=0,
    )

    assert (
        cli.main(
            [
                "--db",
                str(db),
                "--logs-root",
                str(tmp_path / "runs"),
                "--repo-root",
                str(state.ROOT),
                "audit",
                "--no-notebook-check",
                "--max-issues",
                "0",
            ]
        )
        == 0
    )
    output = capsys.readouterr().out
    assert "audit-weak-proof-metadata run=weak-metadata-run" in output
    assert "missing scopes" in output
    assert "resource_family=generic" in output
    assert "contention_key=generic:default" in output
    assert "suspicious reason='\"Prove'" in output


def test_proof_queue_audit_surfaces_product_frontier_before_warning_noise(
    tmp_path: Path, capsys: pytest.CaptureFixture[str]
) -> None:
    db = tmp_path / "proof_queue.sqlite3"
    conn = state._connect(db)
    product_log = tmp_path / "frontier.log"
    warning_log = tmp_path / "guard-warning.log"
    scheduling._insert_run(
        conn,
        run_id="frontier-run",
        logical_id="pact-witness-acceptance",
        reason="prove audit product frontier",
        command=[sys.executable, "-c", "raise SystemExit(2)"],
        cwd=state.ROOT,
        resource_family="wasm-browser",
        contention_key="wasm:pact-witness",
        scopes=["collab/pact/"],
        git_snapshot={
            "available": True,
            "head": "abc123",
            "dirty": False,
            "status": [],
        },
        log_path=product_log,
        summary_json=tmp_path / "frontier.memory_guard.json",
    )
    state._insert_note(
        conn,
        run_id="frontier-run",
        body="test: product frontier must be visible before warning noise",
        kind="submission",
        author="codex",
    )
    product_log.write_text(
        "External static package native-artifact custody errors: "
        "numpy: object_closure runtime ABI symbol "
        "'molt_cpython_abi_date_from_date' is not in the generated WASM "
        "ABI/link import surface\n",
        encoding="utf-8",
    )
    state._update_run(conn, "frontier-run", status="failed", returncode=2)

    scheduling._insert_run(
        conn,
        run_id="guard-warning-run",
        logical_id="guard-warning",
        reason="prove audit warning noise does not hide frontier",
        command=[sys.executable, "-c", "print('ok')"],
        cwd=state.ROOT,
        resource_family="python",
        contention_key="python:guard-warning",
        scopes=["tools/proof_queue.py"],
        git_snapshot={
            "available": True,
            "head": "abc123",
            "dirty": False,
            "status": [],
        },
        log_path=warning_log,
        summary_json=tmp_path / "guard-warning.memory_guard.json",
    )
    state._insert_note(
        conn,
        run_id="guard-warning-run",
        body="test: warning remains visible but secondary",
        kind="submission",
        author="codex",
    )
    warning_log.write_text(
        "memory_guard: orphaned child processes detected after command exit; "
        "killed_at=2026-07-02T00:00:00Z elapsed=1.00s\n",
        encoding="utf-8",
    )
    state._update_run(conn, "guard-warning-run", status="passed", returncode=0)

    assert (
        cli.main(
            [
                "--db",
                str(db),
                "--logs-root",
                str(tmp_path / "runs"),
                "--repo-root",
                str(state.ROOT),
                "audit",
                "--no-notebook-check",
            ]
        )
        == 0
    )
    output = capsys.readouterr().out
    assert "frontier:" in output
    assert "external-native-abi-link-surface-missing run=frontier-run" in output
    assert output.index("frontier:") < output.index("audit-memory-guard-orphan-cleanup")


def test_proof_queue_audit_errors_only_hides_warning_rows_not_errors(
    tmp_path: Path, capsys: pytest.CaptureFixture[str]
) -> None:
    db = tmp_path / "proof_queue.sqlite3"
    conn = state._connect(db)

    error_log = tmp_path / "error.log"
    scheduling._insert_run(
        conn,
        run_id="error-run",
        logical_id="error-run",
        reason="prove audit errors-only still surfaces errors",
        command=[sys.executable, "-c", "raise SystemExit(1)"],
        cwd=state.ROOT,
        resource_family="python",
        contention_key="python:error-run",
        scopes=["tools/proof_queue.py"],
        git_snapshot={
            "available": True,
            "head": "abc123",
            "dirty": False,
            "status": [],
        },
        log_path=error_log,
        summary_json=tmp_path / "error.memory_guard.json",
    )
    state._insert_note(
        conn,
        run_id="error-run",
        body="test: unclassified errors stay visible under errors-only",
        kind="submission",
        author="codex",
    )
    error_log.write_text("mystery failure without a diagnostic\n", encoding="utf-8")
    state._update_run(conn, "error-run", status="failed", returncode=1)

    warning_log = tmp_path / "warning.log"
    scheduling._insert_run(
        conn,
        run_id="warning-run",
        logical_id="warning-run",
        reason="prove audit errors-only filters warning rows",
        command=[sys.executable, "-c", "print('ok')"],
        cwd=state.ROOT,
        resource_family="python",
        contention_key="python:warning-run",
        scopes=["tools/proof_queue.py"],
        git_snapshot={
            "available": True,
            "head": "abc123",
            "dirty": False,
            "status": [],
        },
        log_path=warning_log,
        summary_json=tmp_path / "warning.memory_guard.json",
    )
    state._insert_note(
        conn,
        run_id="warning-run",
        body="test: warning rows are optional human noise",
        kind="submission",
        author="codex",
    )
    warning_log.write_text(
        "memory_guard: orphaned child processes detected after command exit; "
        "killed_at=2026-07-02T00:00:00Z elapsed=1.00s\n",
        encoding="utf-8",
    )
    state._update_run(conn, "warning-run", status="passed", returncode=0)

    assert (
        cli.main(
            [
                "--db",
                str(db),
                "--logs-root",
                str(tmp_path / "runs"),
                "--repo-root",
                str(state.ROOT),
                "audit",
                "--no-notebook-check",
                "--errors-only",
            ]
        )
        == 1
    )
    output = capsys.readouterr().out
    assert "audit-unclassified-failure run=error-run" in output
    assert "audit-memory-guard-orphan-cleanup" not in output
    assert "hidden 1 warning issue(s) due to --errors-only" in output
    assert "issue_severity: error=1, warning=1" in output


def test_proof_queue_audit_omits_superseded_frontier_failures(
    tmp_path: Path, capsys: pytest.CaptureFixture[str]
) -> None:
    db = tmp_path / "proof_queue.sqlite3"
    conn = state._connect(db)
    for run_id, status in (
        ("stale-failure", "failed"),
        ("rerun-child", "passed"),
        ("current-failure", "failed"),
    ):
        log_path = tmp_path / f"{run_id}.log"
        scheduling._insert_run(
            conn,
            run_id=run_id,
            logical_id="pact-witness-acceptance",
            reason="prove superseded frontier filtering",
            command=[sys.executable, "-c", "raise SystemExit(1)"],
            cwd=state.ROOT,
            resource_family="wasm-browser",
            contention_key=f"wasm:{run_id}",
            scopes=["tools/proof_queue.py"],
            git_snapshot={
                "available": True,
                "head": "abc123",
                "dirty": False,
                "status": [],
            },
            log_path=log_path,
            summary_json=tmp_path / f"{run_id}.memory_guard.json",
        )
        state._insert_note(
            conn,
            run_id=run_id,
            body="test: frontier filtering has explicit run context",
            kind="submission",
            author="codex",
        )
        log_path.write_text(
            "External static package native-artifact custody errors: "
            "numpy: object_closure runtime ABI symbol "
            "'molt_cpython_abi_date_from_date' is not in the generated WASM "
            "ABI/link import surface\n",
            encoding="utf-8",
        )
        state._update_run(
            conn,
            run_id,
            status=status,
            returncode=0 if status == "passed" else 1,
        )
    state._insert_edge(
        conn,
        parent_run_id="stale-failure",
        child_run_id="rerun-child",
        kind="reruns",
        note="rerun retired stale frontier",
        author="codex",
    )

    assert (
        cli.main(
            [
                "--db",
                str(db),
                "--logs-root",
                str(tmp_path / "runs"),
                "--repo-root",
                str(state.ROOT),
                "audit",
                "--no-notebook-check",
            ]
        )
        == 0
    )
    output = capsys.readouterr().out
    assert "external-native-abi-link-surface-missing run=current-failure" in output
    assert "run=stale-failure" not in output


def test_proof_queue_audit_omits_superseded_queue_debt_by_default(
    tmp_path: Path, capsys: pytest.CaptureFixture[str]
) -> None:
    db = tmp_path / "proof_queue.sqlite3"
    conn = state._connect(db)
    for run_id, status in (
        ("stale-timeout", "failed"),
        ("rerun-child", "passed"),
    ):
        log_path = tmp_path / f"{run_id}.log"
        scheduling._insert_run(
            conn,
            run_id=run_id,
            logical_id="memory-guard-dx",
            reason="prove superseded queue debt filtering",
            command=[sys.executable, "-c", "print('proof')"],
            cwd=state.ROOT,
            resource_family="python-tests",
            contention_key=f"python:{run_id}",
            scopes=["tools/proof_queue.py"],
            git_snapshot={
                "available": True,
                "head": "abc123",
                "dirty": False,
                "status": [],
            },
            log_path=log_path,
            summary_json=tmp_path / f"{run_id}.memory_guard.json",
        )
        state._insert_note(
            conn,
            run_id=run_id,
            body="test: superseded queue debt remains archaeology, not current health",
            kind="submission",
            author="codex",
        )
        log_path.write_text(
            (
                "memory_guard: timeout after 300.00s; terminated tracked "
                "process tree to prevent orphaned Molt subprocesses\n"
            )
            if run_id == "stale-timeout"
            else "ok\n",
            encoding="utf-8",
        )
        state._update_run(
            conn,
            run_id,
            status=status,
            returncode=0 if status == "passed" else 124,
        )
    state._insert_edge(
        conn,
        parent_run_id="stale-timeout",
        child_run_id="rerun-child",
        kind="reruns",
        note="rerun retired stale timeout",
        author="codex",
    )

    assert (
        cli.main(
            [
                "--db",
                str(db),
                "--logs-root",
                str(tmp_path / "runs"),
                "--repo-root",
                str(state.ROOT),
                "audit",
                "--no-notebook-check",
            ]
        )
        == 0
    )
    output = capsys.readouterr().out
    assert "archaeology: superseded_terminal=1" in output
    assert "classified_failed=0" in output
    assert "diagnostics:" not in output
    assert "run=stale-timeout" not in output

    assert (
        cli.main(
            [
                "--db",
                str(db),
                "--logs-root",
                str(tmp_path / "runs"),
                "--repo-root",
                str(state.ROOT),
                "audit",
                "--all",
                "--no-notebook-check",
            ]
        )
        == 1
    )
    output = capsys.readouterr().out
    assert "classified_failed=1" in output
    assert "diagnostics: memory-guard-timeout=1" in output
    assert "archaeology:" not in output
    assert "audit-memory-guard-timeout run=stale-timeout" in output


def test_proof_queue_audit_only_retires_explicit_live_superseding_children(
    tmp_path: Path, capsys: pytest.CaptureFixture[str]
) -> None:
    cases = (
        ("depends_on", "passed"),
        ("derives_from", "passed"),
        ("compares", "passed"),
        ("reruns", "stale"),
        ("supersedes", "stale"),
    )
    for kind, child_status in cases:
        db = tmp_path / f"{kind}-{child_status}.sqlite3"
        conn = state._connect(db)
        parent_run_id = f"stale-timeout-{kind}-{child_status}"
        child_run_id = f"child-{kind}-{child_status}"
        for run_id, status in (
            (parent_run_id, "failed"),
            (child_run_id, child_status),
        ):
            log_path = tmp_path / f"{run_id}.log"
            scheduling._insert_run(
                conn,
                run_id=run_id,
                logical_id="memory-guard-dx",
                reason="prove audit retirement stays narrow",
                command=[sys.executable, "-c", "print('proof')"],
                cwd=state.ROOT,
                resource_family="python-tests",
                contention_key=f"python:{run_id}",
                scopes=["tools/proof_queue.py"],
                git_snapshot={
                    "available": True,
                    "head": "abc123",
                    "dirty": False,
                    "status": [],
                },
                log_path=log_path,
                summary_json=tmp_path / f"{run_id}.memory_guard.json",
            )
            state._insert_note(
                conn,
                run_id=run_id,
                body="test: audit must not over-retire queue debt",
                kind="submission",
                author="codex",
            )
            log_path.write_text(
                (
                    "memory_guard: timeout after 300.00s; terminated tracked "
                    "process tree to prevent orphaned Molt subprocesses\n"
                )
                if run_id == parent_run_id
                else "ok\n",
                encoding="utf-8",
            )
            state._update_run(
                conn,
                run_id,
                status=status,
                returncode=124 if status == "failed" else 0,
            )
        state._insert_edge(
            conn,
            parent_run_id=parent_run_id,
            child_run_id=child_run_id,
            kind=kind,
            note="edge must not over-retire parent failure",
            author="codex",
        )

        assert (
            cli.main(
                [
                    "--db",
                    str(db),
                    "--logs-root",
                    str(tmp_path / "runs"),
                    "--repo-root",
                    str(state.ROOT),
                    "audit",
                    "--no-notebook-check",
                ]
            )
            == 1
        )
        output = capsys.readouterr().out
        assert "classified_failed=1" in output
        assert "diagnostics: memory-guard-timeout=1" in output
        assert "archaeology:" not in output
        assert f"audit-memory-guard-timeout run={parent_run_id}" in output


def test_proof_queue_audit_fails_on_unclassified_failure(
    tmp_path: Path, capsys: pytest.CaptureFixture[str]
) -> None:
    db = tmp_path / "proof_queue.sqlite3"
    log_path = tmp_path / "mystery.log"
    conn = state._connect(db)
    scheduling._insert_run(
        conn,
        run_id="mystery-run",
        logical_id="mystery",
        reason="prove queue audit catches unclassified rows",
        command=[sys.executable, "-c", "raise SystemExit(1)"],
        cwd=state.ROOT,
        resource_family="python",
        contention_key="python:mystery",
        scopes=["tools/proof_queue.py"],
        git_snapshot={
            "available": True,
            "head": "abc123",
            "dirty": False,
            "status": [],
        },
        log_path=log_path,
        summary_json=tmp_path / "mystery.memory_guard.json",
    )
    state._insert_note(
        conn,
        run_id="mystery-run",
        body="test: unclassified failure must be queue debt",
        kind="submission",
        author="codex",
    )
    log_path.write_text("mystery failure with no known diagnostic\n", encoding="utf-8")
    state._update_run(conn, "mystery-run", status="failed", returncode=1)

    assert (
        cli.main(
            [
                "--db",
                str(db),
                "--logs-root",
                str(tmp_path / "runs"),
                "--repo-root",
                str(state.ROOT),
                "audit",
                "--no-notebook-check",
            ]
        )
        == 1
    )
    output = capsys.readouterr().out
    assert "audit-unclassified-failure" in output
    assert "add a queue diagnostic rule" in output


def test_proof_queue_audit_caps_human_issue_output(
    tmp_path: Path, capsys: pytest.CaptureFixture[str]
) -> None:
    db = tmp_path / "proof_queue.sqlite3"
    conn = state._connect(db)
    for index in range(3):
        run_id = f"mystery-run-{index}"
        log_path = tmp_path / f"{run_id}.log"
        scheduling._insert_run(
            conn,
            run_id=run_id,
            logical_id="mystery",
            reason="prove capped audit output",
            command=[sys.executable, "-c", "raise SystemExit(1)"],
            cwd=state.ROOT,
            resource_family="python",
            contention_key=f"python:mystery:{index}",
            scopes=["tools/proof_queue.py"],
            git_snapshot={
                "available": True,
                "head": "abc123",
                "dirty": False,
                "status": [],
            },
            log_path=log_path,
            summary_json=tmp_path / f"{run_id}.memory_guard.json",
        )
        state._insert_note(
            conn,
            run_id=run_id,
            body="test: unclassified failure must remain visible",
            kind="submission",
            author="codex",
        )
        log_path.write_text(
            "mystery failure with no known diagnostic\n", encoding="utf-8"
        )
        state._update_run(conn, run_id, status="failed", returncode=1)

    assert (
        cli.main(
            [
                "--db",
                str(db),
                "--logs-root",
                str(tmp_path / "runs"),
                "--repo-root",
                str(state.ROOT),
                "audit",
                "--no-notebook-check",
                "--max-issues",
                "2",
            ]
        )
        == 1
    )
    output = capsys.readouterr().out
    assert "diagnostics: unclassified-failed-proof=3" in output
    assert "issue_severity: error=3" in output
    assert "showing 2 of 3 issues" in output


def test_proof_queue_links_runs_and_exports_dag_evidence(
    tmp_path: Path, capsys: pytest.CaptureFixture[str]
) -> None:
    db = tmp_path / "proof_queue.sqlite3"
    notebooks = tmp_path / "notebooks"
    conn = state._connect(db)
    for run_id in ("parent-run", "child-run"):
        scheduling._insert_run(
            conn,
            run_id=run_id,
            logical_id=run_id,
            reason="prove DAG link",
            command=[sys.executable, "-c", "print('dag')"],
            cwd=state.ROOT,
            resource_family="python",
            contention_key=f"python:{run_id}",
            scopes=["tools/proof_queue.py"],
            git_snapshot={
                "available": True,
                "head": "abc123",
                "dirty": False,
                "status": [],
            },
            log_path=tmp_path / f"{run_id}.log",
            summary_json=tmp_path / f"{run_id}.memory_guard.json",
        )

    assert (
        cli.main(
            [
                "--db",
                str(db),
                "--logs-root",
                str(tmp_path / "runs"),
                "--notebooks-root",
                str(notebooks),
                "--repo-root",
                str(state.ROOT),
                "link",
                "child-run",
                "--parent",
                "parent-run",
                "--kind",
                "reruns",
                "--author",
                "codex",
                "--note",
                "Child replays the parent after the import fix.",
            ]
        )
        == 0
    )

    edges = _edges(db)
    assert len(edges) == 1
    assert edges[0]["kind"] == "reruns"
    assert edges[0]["author"] == "codex"
    assert "import fix" in edges[0]["note"]
    child_notebook = (notebooks / "child-run.py").read_text(encoding="utf-8")
    assert '"parent_run_id": "parent-run"' in child_notebook

    capsys.readouterr()
    assert (
        cli.main(
            [
                "--db",
                str(db),
                "--logs-root",
                str(tmp_path / "runs"),
                "--repo-root",
                str(state.ROOT),
                "evidence",
                "--run-id",
                "child-run",
            ]
        )
        == 0
    )
    evidence = json.loads(capsys.readouterr().out)
    assert evidence[0]["dag"]["parent_kind_counts"] == {"reruns": 1}
    assert evidence[0]["dag"]["parents"][0]["parent_run_id"] == "parent-run"


def test_proof_queue_link_projection_failure_preserves_edge(
    monkeypatch: pytest.MonkeyPatch, tmp_path: Path
) -> None:
    db = tmp_path / "proof_queue.sqlite3"
    conn = state._connect(db)
    for run_id in ("parent-warning-run", "child-warning-run"):
        scheduling._insert_run(
            conn,
            run_id=run_id,
            logical_id=run_id,
            reason="prove DAG link survives notebook projection failure",
            command=[sys.executable, "-c", "print('dag')"],
            cwd=state.ROOT,
            resource_family="python",
            contention_key=f"python:{run_id}",
            scopes=["tools/proof_queue.py"],
            git_snapshot={
                "available": True,
                "head": "abc123",
                "dirty": False,
                "status": [],
            },
            log_path=tmp_path / f"{run_id}.log",
            summary_json=tmp_path / f"{run_id}.memory_guard.json",
        )

    def fail_notebook(*_args: object, **_kwargs: object) -> Path:
        raise RuntimeError("link notebook exploded")

    monkeypatch.setattr(evidence_module, "_write_marimo_notebook", fail_notebook)

    assert (
        cli.main(
            [
                "--db",
                str(db),
                "--logs-root",
                str(tmp_path / "runs"),
                "--repo-root",
                str(state.ROOT),
                "link",
                "child-warning-run",
                "--parent",
                "parent-warning-run",
                "--kind",
                "reruns",
                "--note",
                "edge survives projection failure",
            ]
        )
        == 0
    )

    edges = _edges(db)
    assert len(edges) == 1
    assert edges[0]["kind"] == "reruns"
    assert "projection failure" in edges[0]["note"]
    for run_id in ("parent-warning-run", "child-warning-run"):
        log_text = (tmp_path / f"{run_id}.log").read_text(encoding="utf-8")
        assert (
            "proof queue nonfatal infrastructure failure during link projection"
            in log_text
        )
        assert "RuntimeError: link notebook exploded" in log_text


def test_proof_queue_rejects_unknown_note_kind(tmp_path: Path) -> None:
    db = tmp_path / "proof_queue.sqlite3"
    conn = state._connect(db)
    scheduling._insert_run(
        conn,
        run_id="kind-run",
        logical_id="kind",
        reason="prove note kind vocabulary",
        command=[sys.executable, "-c", "print('kind')"],
        cwd=state.ROOT,
        resource_family="python",
        contention_key="python:kind",
        scopes=["tools/proof_queue.py"],
        git_snapshot={
            "available": True,
            "head": "abc123",
            "dirty": False,
            "status": [],
        },
        log_path=tmp_path / "kind.log",
        summary_json=tmp_path / "kind.memory_guard.json",
    )

    with pytest.raises(SystemExit, match="unknown proof note kind"):
        state._insert_note(
            conn,
            run_id="kind-run",
            author="codex",
            kind="blocker",
            body="this vocabulary should fail closed",
        )

    with pytest.raises(sqlite3.DatabaseError, match="unknown proof note kind"):
        conn.execute(
            """
            INSERT INTO proof_notes (run_id, created_at, author, kind, body)
            VALUES (?, ?, ?, ?, ?)
            """,
            (
                "kind-run",
                state._utc_now(),
                "codex",
                "blocker",
                "raw sqlite path should fail closed",
            ),
        )


def test_proof_queue_notes_are_database_append_only(tmp_path: Path) -> None:
    db = tmp_path / "proof_queue.sqlite3"
    conn = state._connect(db)
    scheduling._insert_run(
        conn,
        run_id="append-only-run",
        logical_id="append-only",
        reason="prove immutable notes table",
        command=[sys.executable, "-c", "print('append-only')"],
        cwd=state.ROOT,
        resource_family="python",
        contention_key="python:append-only",
        scopes=["tools/proof_queue.py"],
        git_snapshot={
            "available": True,
            "head": "abc123",
            "dirty": False,
            "status": [],
        },
        log_path=tmp_path / "append-only.log",
        summary_json=tmp_path / "append-only.memory_guard.json",
    )
    state._insert_note(
        conn,
        run_id="append-only-run",
        author="codex",
        kind="observation",
        body="first observation",
    )

    with pytest.raises(sqlite3.DatabaseError, match="append-only"):
        conn.execute("UPDATE proof_notes SET body = 'rewritten'")

    with pytest.raises(sqlite3.DatabaseError, match="append-only"):
        conn.execute("DELETE FROM proof_notes")

    assert [note["body"] for note in _notes(db)] == ["first observation"]


def test_proof_queue_edges_are_append_only_and_acyclic(tmp_path: Path) -> None:
    db = tmp_path / "proof_queue.sqlite3"
    conn = state._connect(db)
    for run_id in ("a-run", "b-run"):
        scheduling._insert_run(
            conn,
            run_id=run_id,
            logical_id=run_id,
            reason="prove DAG guard",
            command=[sys.executable, "-c", "print('dag')"],
            cwd=state.ROOT,
            resource_family="python",
            contention_key=f"python:{run_id}",
            scopes=["tools/proof_queue.py"],
            git_snapshot={
                "available": True,
                "head": "abc123",
                "dirty": False,
                "status": [],
            },
            log_path=tmp_path / f"{run_id}.log",
            summary_json=tmp_path / f"{run_id}.memory_guard.json",
        )
    state._insert_edge(
        conn,
        parent_run_id="a-run",
        child_run_id="b-run",
        kind="depends_on",
        note="b waits on a",
    )

    with pytest.raises(SystemExit, match="would create a cycle"):
        state._insert_edge(
            conn,
            parent_run_id="b-run",
            child_run_id="a-run",
            kind="depends_on",
        )

    with pytest.raises(SystemExit, match="unknown proof edge kind"):
        state._insert_edge(
            conn,
            parent_run_id="a-run",
            child_run_id="b-run",
            kind="blocks",
        )

    with pytest.raises(sqlite3.DatabaseError, match="append-only"):
        conn.execute("UPDATE proof_run_edges SET note = 'rewritten'")

    with pytest.raises(sqlite3.DatabaseError, match="append-only"):
        conn.execute("DELETE FROM proof_run_edges")

    edges = _edges(db)
    assert len(edges) == 1
    assert edges[0]["note"] == "b waits on a"


def test_proof_queue_submit_rejects_uv_run_without_active_project_python(
    tmp_path: Path,
) -> None:
    db = tmp_path / "proof_queue.sqlite3"
    logs = tmp_path / "runs"
    dsl = tmp_path / "proof.toml"
    dsl.write_text(
        "\n".join(
            [
                "[[proof]]",
                'id = "bad-queued-proof"',
                'reason = "reject queued throwaway uv env"',
                'resource_family = "python"',
                'contention_key = "python:bad-queued"',
                'command = ["uv", "run", "python", "-c", "print(\'bad\')"]',
            ]
        ),
        encoding="utf-8",
    )

    with pytest.raises(SystemExit, match="refuses `uv run`"):
        cli.main(
            [
                "--db",
                str(db),
                "--logs-root",
                str(logs),
                "--repo-root",
                str(state.ROOT),
                "submit",
                str(dsl),
            ]
        )


def test_proof_queue_submit_rejects_invalid_memory_guard_poll_env(
    tmp_path: Path,
) -> None:
    db = tmp_path / "proof_queue.sqlite3"
    logs = tmp_path / "runs"
    dsl = tmp_path / "proof.toml"
    dsl.write_text(
        "\n".join(
            [
                "[[proof]]",
                'id = "bad-poll-dsl"',
                'reason = "reject invalid poll interval from DSL"',
                'resource_family = "python"',
                'contention_key = "python:bad-poll-dsl"',
                'env = { MOLT_MEMORY_GUARD_POLL_SEC = "0" }',
                "command = [",
                '  "uv", "run", "--active", "--project", ".",',
                '  "--python", "3.12", "python", "-c", "print(1)",',
                "]",
            ]
        ),
        encoding="utf-8",
    )

    with pytest.raises(SystemExit, match="bad-poll-dsl.*MOLT_MEMORY_GUARD_POLL_SEC"):
        cli.main(
            [
                "--db",
                str(db),
                "--logs-root",
                str(logs),
                "--repo-root",
                str(state.ROOT),
                "submit",
                str(dsl),
            ]
        )
    assert _rows(db) == []


def test_proof_queue_pact_witness_acceptance_is_queue_native(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    monkeypatch.setattr(pact, "_pact_witness_env_overrides", lambda _root: {})
    spec = pact._pact_witness_acceptance_spec()

    assert spec["logical_id"] == "pact-witness-acceptance"
    assert spec["resource_family"] == "wasm-browser"
    assert spec["contention_key"] == "wasm:pact-witness"
    command = list(spec["command"])
    assert command[:7] == [
        "uv",
        "run",
        "--active",
        "--project",
        ".",
        "--python",
        "3.12",
    ]
    stack = resolve_scientific_stack()
    assert command[7:11] == [
        "--with",
        stack.numpy_requirement,
        "--with",
        stack.scipy_requirement,
    ]
    python_index = command.index("python")
    assert command[python_index : python_index + 2] == [
        "python",
        "tools/pact_witness_acceptance.py",
    ]
    assert "tmp/pact_witness_acceptance_queue" in command
    assert "tools/pact_witness_acceptance.py" in spec["scopes"]
    assert spec["env_overrides"]["MOLT_WITNESS_EXPECTED_REPO_ROOT"] == str(
        state.ROOT.resolve()
    )
    assert spec["env_overrides"]["MOLT_WITNESS_EXPECTED_GIT_HEAD"]
    assert "collab/pact/pact_witness_kernel/make_fixture.py" in spec["scopes"]
    assert "collab/pact/pact_witness_kernel/check_parity.py" in spec["scopes"]
    assert any(
        "regenerates the fixture/reference oracle" in note for note in spec["notes"]
    )
    assert any("candidate_outputs.npz" in note for note in spec["notes"])
    assert policy._proof_command_policy_error(command) is None


def test_proof_queue_named_spec_locked_environment_authority_is_generic(
    capsys: pytest.CaptureFixture[str],
) -> None:
    spec = {
        "logical_id": "generic-locked-environment",
        "env_overrides": {"LOCKED_INPUT": "attested"},
        "locked_env": ["LOCKED_INPUT"],
    }
    args = SimpleNamespace(env=["locked_input=user"], print_spec=True)

    with pytest.raises(SystemExit) as exc:
        pact._run_named_spec(args, spec)

    assert exc.value.code == (
        "named proof 'generic-locked-environment' rejects --env overrides for "
        "locked environment custody: LOCKED_INPUT"
    )
    assert capsys.readouterr().out == ""


def test_proof_queue_named_spec_requires_launch_value_for_every_locked_name() -> None:
    spec = {
        "logical_id": "generic-missing-locked-value",
        "env_overrides": {},
        "locked_env": ["LOCKED_INPUT"],
    }
    args = SimpleNamespace(env=[], print_spec=True)

    with pytest.raises(SystemExit, match="without canonical launch values"):
        pact._run_named_spec(args, spec)


@pytest.mark.parametrize(
    "name",
    pact._PACT_WITNESS_ACCEPTANCE_LOCKED_ENV,
)
def test_proof_queue_pact_witness_acceptance_rejects_locked_env_before_print_spec(
    name: str,
    tmp_path: Path,
    capsys: pytest.CaptureFixture[str],
) -> None:
    db = tmp_path / "proof_queue.sqlite3"

    with pytest.raises(SystemExit) as exc:
        cli.main(
            [
                "--db",
                str(db),
                "--logs-root",
                str(tmp_path / "runs"),
                "--repo-root",
                str(state.ROOT),
                "pact-witness-acceptance",
                "--env",
                f"{name}=user-value",
                "--print-spec",
            ]
        )

    message = str(exc.value.code)
    assert "rejects --env overrides for locked environment custody" in message
    assert name in message
    assert "Traceback" not in message
    assert capsys.readouterr().out == ""
    assert not db.exists()


def test_proof_queue_pact_witness_acceptance_rejects_locked_env_before_queue(
    tmp_path: Path,
) -> None:
    db = tmp_path / "proof_queue.sqlite3"

    with pytest.raises(SystemExit, match="MOLT_MODULE_ROOTS"):
        cli.main(
            [
                "--db",
                str(db),
                "--logs-root",
                str(tmp_path / "runs"),
                "--repo-root",
                str(state.ROOT),
                "pact-witness-acceptance",
                "--env",
                "molt_module_roots=user-root",
                "--queue-only",
            ]
        )

    assert not db.exists()


def test_proof_queue_pact_witness_acceptance_allows_diagnostic_env(
    monkeypatch: pytest.MonkeyPatch,
    tmp_path: Path,
    capsys: pytest.CaptureFixture[str],
) -> None:
    db = tmp_path / "proof_queue.sqlite3"
    monkeypatch.setattr(pact, "_pact_witness_native_roots", lambda _root: [])

    assert (
        cli.main(
            [
                "--db",
                str(db),
                "--logs-root",
                str(tmp_path / "runs"),
                "--repo-root",
                str(state.ROOT),
                "pact-witness-acceptance",
                "--env",
                "MOLT_TRACE_CAPI=1",
                "--env",
                "MOLT_TRACE_IMPORT_STAGE=1",
                "--print-spec",
            ]
        )
        == 0
    )
    spec = json.loads(capsys.readouterr().out)
    assert spec["env_overrides"]["MOLT_TRACE_CAPI"] == "1"
    assert spec["env_overrides"]["MOLT_TRACE_IMPORT_STAGE"] == "1"
    assert set(spec["locked_env"]) == set(pact._PACT_WITNESS_ACCEPTANCE_LOCKED_ENV)
    assert not db.exists()


def test_proof_queue_pact_witness_acceptance_scrubs_ambient_input_redirects(
    monkeypatch: pytest.MonkeyPatch,
    tmp_path: Path,
    capsys: pytest.CaptureFixture[str],
) -> None:
    monkeypatch.setenv("MOLT_SCIENTIFIC_STACK_CONFIG", str(tmp_path / "host.toml"))
    monkeypatch.setenv("MOLT_EXT_ROOT", str(tmp_path / "host-artifacts"))
    monkeypatch.setenv(
        "MOLT_EXTERNAL_ARTIFACT_ROOTS", str(tmp_path / "other-artifacts")
    )
    monkeypatch.setattr(pact, "_pact_witness_native_roots", lambda _root: [])

    assert (
        cli.main(
            [
                "--db",
                str(tmp_path / "proof_queue.sqlite3"),
                "--logs-root",
                str(tmp_path / "runs"),
                "--repo-root",
                str(state.ROOT),
                "pact-witness-acceptance",
                "--print-spec",
            ]
        )
        == 0
    )

    spec = json.loads(capsys.readouterr().out)
    canonical = pact._pact_canonical_input_environment(state.ROOT)
    for name, value in canonical.items():
        assert spec["env_overrides"][name] == value
    assert str(tmp_path) not in json.dumps(spec["env_overrides"])


def test_pact_canonical_input_root_is_independent_of_build_free_space(
    monkeypatch: pytest.MonkeyPatch,
    tmp_path: Path,
) -> None:
    selected = tmp_path / "canonical-inputs"
    observed: list[Path] = []

    def custody(repo_root: Path) -> Path:
        observed.append(repo_root)
        return selected

    monkeypatch.setattr(
        pact,
        "checkout_custody",
        lambda repo_root, _env: SimpleNamespace(custody_root=custody(repo_root)),
    )

    canonical = pact._pact_canonical_input_environment(state.ROOT)

    assert observed == [state.ROOT.resolve()]
    assert canonical["MOLT_EXT_ROOT"] == str(selected.resolve())
    assert canonical["MOLT_EXTERNAL_ARTIFACT_ROOTS"] == str(selected.resolve())


def test_proof_queue_prune_stale_explicitly_cancels_selected_queued_row(
    tmp_path: Path,
    capsys: pytest.CaptureFixture[str],
) -> None:
    db = tmp_path / "proof_queue.sqlite3"
    conn = state._connect(db)
    scheduling._insert_run(
        conn,
        run_id="abandoned-queued-run",
        logical_id="abandoned",
        reason="prove explicit queued cancellation",
        command=[sys.executable, "-c", "print('never launched')"],
        cwd=state.ROOT,
        resource_family="python-tests",
        contention_key="proof-queue-dx-abandoned",
        scopes=["tools/proof_queue.py"],
        log_path=tmp_path / "abandoned.log",
        summary_json=tmp_path / "abandoned.memory_guard.json",
    )

    assert (
        cli.main(
            [
                "--db",
                str(db),
                "--repo-root",
                str(state.ROOT),
                "prune-stale",
                "--run-id",
                "abandoned-queued-run",
            ]
        )
        == 0
    )

    output = capsys.readouterr().out
    assert "selected-queued-cancellation" in output
    assert "pruned=1" in output
    row = _rows(db)[0]
    assert row["status"] == "stale"
    assert row["returncode"] == custody.PROOF_QUEUE_STALE_EXIT_CODE


def test_proof_queue_r6_target_version_parity_is_queue_native() -> None:
    spec = pact._r6_target_version_parity_spec("3.12")

    assert spec["logical_id"] == "r6-target-version-parity-py312"
    assert spec["resource_family"] == "python"
    assert spec["contention_key"] == "python:r6-target-version-py312"
    command = list(spec["command"])
    assert command[:7] == [
        "uv",
        "run",
        "--active",
        "--project",
        ".",
        "--python",
        "3.12",
    ]
    assert command[7:9] == ["python", "tests/molt_diff.py"]
    assert command[command.index("--python-version") + 1] == "3.12"
    assert command[command.index("--jobs") + 1] == "1"
    assert "--fail-fast" in command
    assert "tests/differential/stdlib/sys_metadata_intrinsics.py" in command
    assert "tests/differential/stdlib/queue_shutdown_version_gate.py" in command
    assert "tests/differential/stdlib/removed_stdlib_modules_version_gate.py" in command
    assert "tools/target_python_runtime.py" in spec["scopes"]
    assert "src/molt/stdlib/sys.py" in spec["scopes"]
    assert "src/molt/stdlib/_sys_impl.py" not in spec["scopes"]
    assert "src/molt/stdlib/queue.py" in spec["scopes"]
    assert any(
        "serial fail-fast differential custody" in note for note in spec["notes"]
    )
    assert any("Selected R6 fixtures:" in note for note in spec["notes"])
    assert any("missing target interpreters" in note for note in spec["notes"])
    assert policy._proof_command_policy_error(command) is None


def test_proof_queue_r6_target_version_parity_can_select_fixture_subset() -> None:
    spec = pact._r6_target_version_parity_spec(
        "3.12",
        fixtures=[
            "removed_stdlib_modules_version_gate",
            "tests/differential/stdlib/sys_metadata_intrinsics.py",
            "removed_stdlib_modules_version_gate.py",
        ],
    )
    command = list(spec["command"])

    assert (
        spec["logical_id"]
        == "r6-target-version-parity-py312-removed-stdlib-modules-version-gate-"
        "sys-metadata-intrinsics"
    )
    assert command[-2:] == [
        "tests/differential/stdlib/removed_stdlib_modules_version_gate.py",
        "tests/differential/stdlib/sys_metadata_intrinsics.py",
    ]
    assert "tests/differential/stdlib/queue_shutdown_version_gate.py" not in command
    assert (
        "tests/differential/stdlib/removed_stdlib_modules_version_gate.py"
        in spec["scopes"]
    )
    assert (
        "tests/differential/stdlib/queue_shutdown_version_gate.py" not in spec["scopes"]
    )
    assert any(
        "tests/differential/stdlib/removed_stdlib_modules_version_gate.py" in note
        for note in spec["notes"]
    )


def test_proof_queue_r6_target_version_parity_rejects_unknown_fixture() -> None:
    with pytest.raises(SystemExit) as exc:
        pact._r6_target_version_parity_spec(
            "3.12",
            fixtures=["queue_shutdown_version_gate.py", "not_a_fixture"],
        )

    assert "unknown R6 target-version fixture 'not_a_fixture'" in str(exc.value)
    assert "removed_stdlib_modules_version_gate.py" in str(exc.value)


def test_proof_queue_r6_target_version_parity_uses_target_tag() -> None:
    spec = pact._r6_target_version_parity_spec("3.13")
    command = list(spec["command"])

    assert spec["logical_id"] == "r6-target-version-parity-py313"
    assert spec["contention_key"] == "python:r6-target-version-py313"
    assert command[command.index("--python-version") + 1] == "3.13"


def test_proof_queue_native_molt_run_is_queue_native(tmp_path: Path) -> None:
    entry = tmp_path / "tmp" / "probe.py"
    entry.parent.mkdir()
    entry.write_text("print('ok')\n", encoding="utf-8")

    spec = pact._native_molt_run_spec(
        "tmp/probe.py",
        script_args=["--", "--flag"],
        repo_root=tmp_path,
    )

    assert spec["logical_id"].startswith("native-molt-run-tmp-probe-py-")
    assert spec["resource_family"] == "python-native"
    assert spec["contention_key"] == "python:native-molt-run:tmp-probe-py"
    command = list(spec["command"])
    assert command[:9] == [
        "uv",
        "run",
        "--active",
        "--project",
        ".",
        "--python",
        "3.12",
        "--no-sync",
        "python",
    ]
    assert command[9:13] == ["-m", "molt.cli", "run", "tmp/probe.py"]
    assert command[-1] == "--flag"
    assert spec["scopes"] == ["tmp/probe.py"]
    assert any("foreground Codex control plane" in note for note in spec["notes"])
    assert policy._proof_command_policy_error(command) is None


def test_proof_queue_native_molt_run_rejects_outside_repo(tmp_path: Path) -> None:
    outside = tmp_path.parent / f"{tmp_path.name}_outside_probe.py"
    outside.write_text("print('outside')\n", encoding="utf-8")

    with pytest.raises(SystemExit, match="must live under repo root"):
        pact._native_molt_run_spec(str(outside), repo_root=tmp_path)


def _publish_scientific_fixture_payload(
    payload_root: Path,
    destination: Path,
    transaction_root: Path,
) -> None:
    if transaction_root.exists():
        shutil.rmtree(transaction_root)
    seal = stage_source_package_seal(
        transaction_root,
        [
            SourcePackageInput(
                path,
                path.relative_to(payload_root).as_posix(),
                "fixture",
            )
            for path in sorted(payload_root.rglob("*"))
            if path.is_file()
        ],
    )
    if destination.exists():
        shutil.rmtree(destination)
    shutil.copytree(seal.root, destination)
    shutil.rmtree(transaction_root)


def _reseal_scientific_fixture(destination: Path) -> None:
    payload_root = destination / "files"
    temporary_payload = destination.parent / f".{destination.name}.reseal-payload"
    transaction_root = destination.parent / f".{destination.name}.reseal-transaction"
    if temporary_payload.exists():
        shutil.rmtree(temporary_payload)
    shutil.copytree(payload_root, temporary_payload)
    _publish_scientific_fixture_payload(
        temporary_payload, destination, transaction_root
    )
    shutil.rmtree(temporary_payload)


def _write_current_scientific_seal(
    root: Path,
    *,
    package: str = "scipy",
    missing_module: str | None = None,
    exports_override: dict[str, list[str]] | None = None,
) -> str | None:
    destination = root
    root = destination.parent / f".{destination.name}.fixture-payload"
    transaction_root = destination.parent / f".{destination.name}.fixture-transaction"
    if root.exists():
        shutil.rmtree(root)
    if transaction_root.exists():
        shutil.rmtree(transaction_root)
    extension_set = scientific_extension_set(package, "pact-witness")
    stack = resolve_scientific_stack()
    current_abi = pact._default_molt_c_api_version(state.ROOT)
    current_abi_tag = f"molt_abi{current_abi.split('.', 1)[0]}"
    set_extensions: list[dict[str, object]] = []
    for extension in extension_set.extensions:
        artifact_name = f"{extension.target}.molt.wasm"
        artifact_bytes = b"\x00asm" + extension.target.encode("utf-8")
        artifact_sha256 = hashlib.sha256(artifact_bytes).hexdigest()
        wheel_sha256 = hashlib.sha256(f"wheel:{extension.target}".encode()).hexdigest()
        source_bytes = f"/* {extension.module} */\n".encode()
        source_sha256 = hashlib.sha256(source_bytes).hexdigest()
        init_symbol = f"PyInit_{extension.module.rsplit('.', 1)[-1]}"
        package_dir = root.joinpath(*extension.module.split(".")[:-1])
        source_path = root.joinpath(
            "provenance", "compiled-inputs", *extension.module.split("."), "source.c"
        )
        source_reference = os.path.relpath(source_path, package_dir).replace(
            os.sep, "/"
        )
        object_closure: dict[str, object] = {
            "schema_version": 1,
            "root_symbol": init_symbol,
            "init_symbol_owner": "0.o",
            "runtime_symbols": [],
            "objects": [
                {
                    "source": source_reference,
                    "object": "0.o",
                    "source_sha256": source_sha256,
                    "object_sha256": "2" * 64,
                    "defined_symbols": [init_symbol],
                    "undefined_symbols": [],
                    "compile_command": [
                        "@llvm-bin/clang",
                        "--target=wasm32-wasip1",
                        "-c",
                        source_reference,
                    ],
                    "symbol_command": ["@llvm-bin/llvm-nm"],
                    "dependencies": [],
                }
            ],
        }
        closure_sha256 = pact._pact_object_closure_digest(
            {"object_closure": object_closure}, object_closure
        )
        assert closure_sha256 is not None
        object_closure["closure_sha256"] = closure_sha256
        set_extensions.append(
            {
                "module": extension.module,
                "target": extension.target,
                "python_exports": list(extension.python_exports),
                "capabilities": list(extension.capabilities),
                "provided_capsules": list(extension.provided_capsules),
                "exclude_linked_static_libraries": list(
                    extension.exclude_linked_static_libraries
                ),
                "artifact_sha256": artifact_sha256,
                "wheel_sha256": wheel_sha256,
                "object_closure_sha256": closure_sha256,
            }
        )
        if extension.module == missing_module:
            continue
        package_dir.mkdir(parents=True, exist_ok=True)
        source_path.parent.mkdir(parents=True, exist_ok=True)
        source_path.write_bytes(source_bytes)
        (package_dir / artifact_name).write_bytes(artifact_bytes)
        wheel_path = root.joinpath(
            "provenance",
            "wheels",
            *extension.module.split("."),
            f"{extension.target}.whl",
        )
        wheel_path.parent.mkdir(parents=True, exist_ok=True)
        wheel_path.write_bytes(f"wheel:{extension.target}".encode())
        manifest_path = package_dir / f"{artifact_name}.extension_manifest.json"
        manifest_path.write_text(
            json.dumps(
                {
                    "module": extension.module,
                    "extension": artifact_name,
                    "extension_sha256": artifact_sha256,
                    "wheel": os.path.relpath(wheel_path, package_dir).replace(
                        os.sep, "/"
                    ),
                    "wheel_sha256": wheel_sha256,
                    "molt_c_api_version": current_abi,
                    "abi_tag": current_abi_tag,
                    "loader_kind": "libmolt_source",
                    "target_triple": "wasm32-wasip1",
                    "runtime_linkage": "static_link",
                    "artifact_kind": "wasm_relocatable_object",
                    "deterministic": True,
                    "capabilities": list(extension.capabilities),
                    "init_symbol": init_symbol,
                    "source_plan": {"target_selector": extension.target},
                    "object_closure": object_closure,
                    "python_exports": (exports_override or {}).get(
                        extension.module, list(extension.python_exports)
                    ),
                    "provided_capsules": list(extension.provided_capsules),
                }
            ),
            encoding="utf-8",
        )
    installed_package_files = sorted(
        {*extension_set.required_installed_files, f"{package}/_fixture_extra.py"}
    )
    for relative in installed_package_files:
        path = root / relative
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_text(f"# {relative}\n", encoding="utf-8")
    target_root = root / "provenance/metadata/target"
    python_pc = target_root / "pkgconfig/python3.pc"
    meson_cross = target_root / "meson.cross"
    python_pc.parent.mkdir(parents=True, exist_ok=True)
    python_pc.write_text("prefix=@molt\n", encoding="utf-8")
    meson_cross.write_text("[binaries]\n", encoding="utf-8")
    tool_names = {
        "cc": "@llvm-bin/clang",
        "cxx": "@llvm-bin/clang++",
        "wasm_ld": "@llvm-bin/wasm-ld",
        "ar": "@llvm-bin/llvm-ar",
        "ranlib": "@llvm-bin/llvm-ranlib",
        "nm": "@llvm-bin/llvm-nm",
        "strip": "@llvm-bin/llvm-strip",
    }
    target_metadata: dict[str, object] = {
        "schema_version": 2,
        "toolchain": {
            "tools": {
                role: {
                    "command": [name],
                    "path": name,
                    "version": "test",
                    "sha256": "a" * 64,
                }
                for role, name in tool_names.items()
            },
            "commands": {
                "c": ["@llvm-bin/clang", "--target=wasm32-wasip1"],
                "cpp": ["@llvm-bin/clang++", "--target=wasm32-wasip1"],
                "ld": ["@llvm-bin/wasm-ld"],
                "ar": ["@llvm-bin/llvm-ar"],
                "ranlib": ["@llvm-bin/llvm-ranlib"],
                "nm": ["@llvm-bin/llvm-nm"],
                "strip": ["@llvm-bin/llvm-strip"],
            },
        },
        "digests": {
            "python_pc_sha256": _sha256_file(python_pc),
            "meson_cross_sha256": _sha256_file(meson_cross),
        },
    }
    target_metadata["digest"] = hashlib.sha256(
        json.dumps(
            target_metadata,
            sort_keys=True,
            separators=(",", ":"),
        ).encode()
    ).hexdigest()
    (target_root / "source-extension-target-metadata.json").write_text(
        json.dumps(target_metadata), encoding="utf-8"
    )
    meson_metadata_root = root / "provenance/metadata/meson"
    meson_metadata_root.mkdir(parents=True, exist_ok=True)
    intro_targets = meson_metadata_root / "intro-targets.json"
    compile_commands = meson_metadata_root / "compile-commands.json"
    intro_installed = meson_metadata_root / "intro-installed.json"
    intro_targets.write_text("[]\n", encoding="utf-8")
    compile_commands.write_text("[]\n", encoding="utf-8")
    intro_installed.write_text("{}\n", encoding="utf-8")
    config_tool_cross = meson_metadata_root / "build-config-tools.cross"
    if extension_set.use_pkg_config:
        config_tool_cross.write_text("[binaries]\n", encoding="utf-8")
    build_custody_python = {
        "implementation": sys.implementation.name,
        "version": (
            f"{sys.version_info.major}.{sys.version_info.minor}."
            f"{sys.version_info.micro}"
        ),
        "platform": "test-platform",
        "base_executable": Path(sys.executable).name,
        "base_executable_sha256": "b" * 64,
    }
    build_custody_address = {
        "schema_version": 2,
        "dependency_group": extension_set.build_dependency_group,
        "dependency_group_requirements": ["ninja==1.13.0"],
        "uv_lock_sha256": "c" * 64,
        "python": build_custody_python,
        "uv": {
            "executable": "uv.exe",
            "version": "uv 0.11.24",
            "sha256": "d" * 64,
        },
    }
    build_custody = {
        "environment_id": hashlib.sha256(
            json.dumps(
                build_custody_address, sort_keys=True, separators=(",", ":")
            ).encode()
        ).hexdigest(),
        **build_custody_address,
    }
    (root / "extension_set_manifest.json").write_text(
        json.dumps(
            {
                "schema_version": 2,
                "kind": "molt-source-extension-set",
                "package": package,
                "name": "pact-witness",
                "seal_name": extension_set.seal_name,
                "source_head": {
                    "numpy": stack.numpy_repo_ref,
                    "scipy": stack.scipy_repo_ref,
                }[package],
                "submodules": [],
                "target": "wasm",
                "target_triple": "wasm32-wasip1",
                "abi_tier": "cpython-abi",
                "target_metadata": target_metadata,
                "build_environment": {
                    "python": {
                        "implementation": sys.implementation.name,
                        "version": (
                            f"{sys.version_info.major}.{sys.version_info.minor}."
                            f"{sys.version_info.micro}"
                        ),
                        "executable": Path(sys.executable).name,
                    },
                    "requirements": [
                        "meson>=1.5",
                        "Cython>=3.0",
                        "pybind11>=2.13.2",
                        "pythran>=0.14.0",
                        "numpy>=2.0.0",
                        'ninja; python_version < "3"',
                    ],
                    "marker_environment": canonical_source_marker_environment(),
                    "active_requirements": [
                        "meson>=1.5",
                        "Cython>=3.0",
                        "pybind11>=2.13.2",
                        "pythran>=0.14.0",
                        "numpy>=2.0.0",
                    ],
                    "resolved": [
                        {
                            "requirement": "meson>=1.5",
                            "distribution": "meson",
                            "version": "1.9.0",
                        },
                        {
                            "requirement": "Cython>=3.0",
                            "distribution": "Cython",
                            "version": "3.2.8",
                        },
                        {
                            "requirement": "pybind11>=2.13.2",
                            "distribution": "pybind11",
                            "version": "3.0.4",
                        },
                        {
                            "requirement": "pythran>=0.14.0",
                            "distribution": "pythran",
                            "version": "0.18.1",
                        },
                        {
                            "requirement": "numpy>=2.0.0",
                            "distribution": "numpy",
                            "version": "2.5.1",
                        },
                    ],
                    "custody": build_custody,
                },
                "meson": {
                    "driver": {
                        "kind": "build-environment",
                        "module": "mesonbuild.mesonmain",
                        "distribution": "meson",
                        "version": "1.9.0",
                    },
                    "backend": {
                        "distribution": "ninja",
                        "version": "1.13.0",
                        "path": "ninja.exe",
                        "sha256": "b" * 64,
                    },
                    "build_root": "@build",
                    "setup_args": list(extension_set.meson_setup_args),
                    "intro_targets_sha256": _sha256_file(intro_targets),
                    "compile_commands_sha256": _sha256_file(compile_commands),
                    "intro_installed_sha256": _sha256_file(intro_installed),
                    "config_tool_cross_sha256": (
                        _sha256_file(config_tool_cross)
                        if extension_set.use_pkg_config
                        else None
                    ),
                    "config_tools": (
                        [
                            {
                                "name": "numpy-config",
                                "path": "numpy-config.exe",
                                "distribution": "numpy",
                                "version": "2.5.1",
                                "sha256": "7" * 64,
                            },
                            {
                                "name": "pkg-config",
                                "path": "pkg-config.exe",
                                "distribution": "pkgconf",
                                "version": "3.0.1.post0",
                                "sha256": "8" * 64,
                            },
                            {
                                "name": "pybind11-config",
                                "path": "pybind11-config.exe",
                                "distribution": "pybind11",
                                "version": "3.0.4",
                                "sha256": "9" * 64,
                            },
                            {
                                "name": "pythran-config",
                                "path": "pythran-config.exe",
                                "distribution": "pythran",
                                "version": "0.18.1",
                                "sha256": "a" * 64,
                            },
                        ]
                        if extension_set.use_pkg_config
                        else []
                    ),
                    "pkg_config_requirement": (
                        pact.MOLT_PKGCONF_REQUIREMENT
                        if extension_set.use_pkg_config
                        else None
                    ),
                    "generated_inputs": [],
                },
                "installed_package_files": installed_package_files,
                "extensions": set_extensions,
            }
        ),
        encoding="utf-8",
    )
    _publish_scientific_fixture_payload(root, destination, transaction_root)
    shutil.rmtree(root)
    seal = verify_source_package_seal(destination)
    try:
        identity = _source_extension_set_identity(
            seal.payload_root,
            inventory_sha256={
                entry.relative_path: entry.sha256 for entry in seal.files
            },
        )
    except ValueError:
        return None
    return str(identity["canonical_sha256"])


def _patch_pact_expected_identities(
    monkeypatch: pytest.MonkeyPatch,
    identities: dict[str, str],
    *,
    artifact_root: Path,
) -> None:
    real_scientific_extension_set = pact.scientific_extension_set

    def resolve(package: str, name: str, *args, **kwargs):
        extension_set = real_scientific_extension_set(package, name, *args, **kwargs)
        expected = identities.get(package)
        return (
            replace(extension_set, expected_identity_sha256=expected)
            if expected is not None
            else extension_set
        )

    monkeypatch.setattr(pact, "scientific_extension_set", resolve)
    monkeypatch.setattr(scientific_versions, "scientific_extension_set", resolve)

    def extension_set_root(extension_set, stack=None):
        selected = resolve_scientific_stack() if stack is None else stack
        version = {"numpy": selected.numpy, "scipy": selected.scipy}[
            extension_set.package
        ]
        return (
            artifact_root
            / "package-seals"
            / extension_set.package
            / version
            / extension_set.seal_name
        )

    monkeypatch.setattr(pact, "scientific_extension_set_root", extension_set_root)


def test_proof_queue_r6_target_version_parity_print_spec(
    tmp_path: Path, capsys: pytest.CaptureFixture[str]
) -> None:
    db = tmp_path / "proof_queue.sqlite3"

    assert (
        cli.main(
            [
                "--db",
                str(db),
                "--logs-root",
                str(tmp_path / "runs"),
                "--repo-root",
                str(state.ROOT),
                "r6-target-version-parity",
                "--python-version",
                "3.13",
                "--fixture",
                "removed_stdlib_modules_version_gate.py",
                "--print-spec",
            ]
        )
        == 0
    )
    spec = json.loads(capsys.readouterr().out)
    assert (
        spec["logical_id"]
        == "r6-target-version-parity-py313-removed-stdlib-modules-version-gate"
    )
    assert spec["command"][spec["command"].index("--python-version") + 1] == "3.13"
    assert "--fail-fast" in spec["command"]
    assert spec["command"][-1] == (
        "tests/differential/stdlib/removed_stdlib_modules_version_gate.py"
    )
    assert spec["resource_family"] == "python"


def test_proof_queue_pact_witness_acceptance_admits_staged_native_roots(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    monkeypatch.setenv("MOLT_EXT_ROOT", str(tmp_path / "artifacts"))
    monkeypatch.setattr(
        pact,
        "_pact_canonical_input_environment",
        lambda _root: {
            pact.SCIENTIFIC_STACK_CONFIG_ENV: str(
                state.ROOT / "config/scientific_stack_versions.toml"
            ),
            "MOLT_EXT_ROOT": str(tmp_path / "artifacts"),
            "MOLT_EXTERNAL_ARTIFACT_ROOTS": str(tmp_path / "artifacts"),
        },
    )
    expected_seals = [
        tmp_path
        / "artifacts/package-seals/numpy/2.5.1/pact_numpy_multiarray_sealed_for_witness",
        tmp_path / "artifacts/package-seals/scipy/1.18.0/pact_scipy_witness",
    ]
    legacy_roots = [
        tmp_path / "tmp/pact_numpy_multiarray_sealed_axiserror",
        tmp_path / "tmp/pact_scipy_ndimage_sealed_for_witness_next",
        tmp_path / "tmp/pact_scipy_ni_label_molt_ext_wasm_cpython_abi",
        tmp_path / "tmp/pact_scipy_ndimage_provider_sealed_support_closure",
        tmp_path / "bench/friends/repos/numpy_off_the_shelf",
        tmp_path / "bench/friends/repos/scipy_off_the_shelf",
    ]
    for root in expected_seals:
        root.mkdir(parents=True)
    for root in legacy_roots:
        root.mkdir(parents=True)
    numpy_identity = _write_current_scientific_seal(expected_seals[0], package="numpy")
    scipy_identity = _write_current_scientific_seal(expected_seals[1])
    assert numpy_identity is not None and scipy_identity is not None
    _patch_pact_expected_identities(
        monkeypatch,
        {"numpy": numpy_identity, "scipy": scipy_identity},
        artifact_root=tmp_path / "artifacts",
    )
    for root in legacy_roots[:4]:
        (root / "extension_manifest.json").write_text("{}", encoding="utf-8")

    spec = pact._pact_witness_acceptance_spec(repo_root=tmp_path)
    env = spec["env_overrides"]

    assert env["MOLT_EXTERNAL_STATIC_PACKAGES"] == "numpy scipy"
    assert env["MOLT_MODULE_ROOTS"].split(os.pathsep) == [
        str((expected_seals[0] / "files").resolve()),
        str((expected_seals[1] / "files").resolve()),
    ]
    assert any("canonical scientific extension seals" in note for note in spec["notes"])


def test_proof_queue_pact_witness_acceptance_fails_when_canonical_scipy_is_absent(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    durable_numpy = (
        tmp_path
        / "artifacts/package-seals/numpy/2.5.1/pact_numpy_multiarray_sealed_for_witness"
    )
    numpy_identity = _write_current_scientific_seal(durable_numpy, package="numpy")
    assert numpy_identity is not None
    _patch_pact_expected_identities(
        monkeypatch,
        {"numpy": numpy_identity},
        artifact_root=tmp_path / "artifacts",
    )
    monkeypatch.setenv("MOLT_EXT_ROOT", str(tmp_path / "artifacts"))

    with pytest.raises(ValueError, match="canonical SciPy witness seal is absent"):
        pact._pact_witness_native_roots(repo_root=tmp_path)


def test_proof_queue_pact_witness_acceptance_rejects_incomplete_scipy_set(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    durable_numpy = (
        tmp_path
        / "artifacts/package-seals/numpy/2.5.1/pact_numpy_multiarray_sealed_for_witness"
    )
    durable_scipy = tmp_path / "artifacts/package-seals/scipy/1.18.0/pact_scipy_witness"
    numpy_identity = _write_current_scientific_seal(durable_numpy, package="numpy")
    _write_current_scientific_seal(
        durable_scipy, missing_module="scipy.ndimage._rank_filter_1d"
    )
    assert numpy_identity is not None
    _patch_pact_expected_identities(
        monkeypatch,
        {"numpy": numpy_identity},
        artifact_root=tmp_path / "artifacts",
    )
    monkeypatch.setenv("MOLT_EXT_ROOT", str(tmp_path / "artifacts"))

    with pytest.raises(ValueError, match=r"absent or incomplete.*_rank_filter_1d"):
        pact._pact_witness_native_roots(repo_root=tmp_path)


def test_proof_queue_pact_witness_acceptance_rejects_scipy_export_drift(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    durable_numpy = (
        tmp_path
        / "artifacts/package-seals/numpy/2.5.1/pact_numpy_multiarray_sealed_for_witness"
    )
    durable_scipy = tmp_path / "artifacts/package-seals/scipy/1.18.0/pact_scipy_witness"
    numpy_identity = _write_current_scientific_seal(durable_numpy, package="numpy")
    scipy_identity = _write_current_scientific_seal(
        durable_scipy,
        exports_override={"scipy.ndimage._nd_image": ["scipy.ndimage"]},
    )
    assert numpy_identity is not None and scipy_identity is not None
    _patch_pact_expected_identities(
        monkeypatch,
        {"numpy": numpy_identity, "scipy": scipy_identity},
        artifact_root=tmp_path / "artifacts",
    )
    monkeypatch.setenv("MOLT_EXT_ROOT", str(tmp_path / "artifacts"))

    with pytest.raises(ValueError, match="absent or incomplete"):
        pact._pact_witness_native_roots(repo_root=tmp_path)


def test_proof_queue_rejects_expected_extension_identity_drift(tmp_path: Path) -> None:
    root = tmp_path / "pact_scipy_witness"
    identity = _write_current_scientific_seal(root)
    assert identity is not None
    extension_set = replace(
        scientific_extension_set("scipy", "pact-witness"),
        expected_identity_sha256="0" * 64,
    )

    problems = pact._scientific_extension_set_seal_problems(root, extension_set)

    assert any(
        "expected canonical extension identity guard failed" in problem
        for problem in problems
    )


@pytest.mark.parametrize(
    ("field", "value", "problem"),
    [
        ("molt_c_api_version", "0", "stale molt_c_api_version"),
        ("abi_tag", "molt_abi0", "stale abi_tag"),
        ("loader_kind", "legacy", "loader_kind must be libmolt_source"),
        ("target_triple", "host", "target_triple must be wasm32-wasip1"),
        ("runtime_linkage", "host_resolved", "runtime_linkage must be static_link"),
        (
            "artifact_kind",
            "shared_library",
            "artifact_kind must be wasm_relocatable_object",
        ),
        ("deterministic", False, "deterministic must be true"),
        ("init_symbol", "PyInit_wrong", "init_symbol mismatch"),
        (
            "source_plan",
            {"target_selector": "wrong"},
            "Meson source_plan target mismatch",
        ),
        ("capabilities", ["fs.read"], "capabilities drift"),
        ("extension_sha256", "0" * 64, "extension_sha256 mismatch"),
        ("object_closure", {"objects": []}, "object_closure is empty"),
    ],
)
def test_proof_queue_rejects_scipy_seal_contract_drift(
    tmp_path: Path, field: str, value: object, problem: str
) -> None:
    root = tmp_path / "pact_scipy_witness"
    _write_current_scientific_seal(root)
    extension_set = scientific_extension_set("scipy", "pact-witness")
    extension = extension_set.extensions[0]
    manifest_path = pact._scientific_extension_manifest_path(
        root / "files", extension.module, extension.target
    )
    manifest = json.loads(manifest_path.read_text(encoding="utf-8"))
    manifest[field] = value
    manifest_path.write_text(json.dumps(manifest), encoding="utf-8")
    _reseal_scientific_fixture(root)

    problems = pact._scientific_extension_set_seal_problems(root, extension_set)

    assert any(problem in item for item in problems), problems


def test_proof_queue_requires_explicit_scipy_determinism_attestation(
    tmp_path: Path,
) -> None:
    root = tmp_path / "pact_scipy_witness"
    _write_current_scientific_seal(root)
    extension_set = scientific_extension_set("scipy", "pact-witness")
    extension = extension_set.extensions[0]
    manifest_path = pact._scientific_extension_manifest_path(
        root / "files", extension.module, extension.target
    )
    manifest = json.loads(manifest_path.read_text(encoding="utf-8"))
    del manifest["deterministic"]
    manifest_path.write_text(json.dumps(manifest), encoding="utf-8")
    _reseal_scientific_fixture(root)

    problems = pact._scientific_extension_set_seal_problems(root, extension_set)

    assert any("deterministic must be true" in item for item in problems), problems


@pytest.mark.parametrize(
    ("field", "value", "problem"),
    [
        ("root_symbol", "PyInit_wrong", "root_symbol mismatch"),
        ("init_symbol_owner", "", "init_symbol_owner is empty"),
        ("closure_sha256", "", "object_closure checksum mismatch"),
    ],
)
def test_proof_queue_rejects_scipy_object_closure_identity_drift(
    tmp_path: Path, field: str, value: object, problem: str
) -> None:
    root = tmp_path / "pact_scipy_witness"
    _write_current_scientific_seal(root)
    extension_set = scientific_extension_set("scipy", "pact-witness")
    extension = extension_set.extensions[0]
    manifest_path = pact._scientific_extension_manifest_path(
        root / "files", extension.module, extension.target
    )
    manifest = json.loads(manifest_path.read_text(encoding="utf-8"))
    manifest["object_closure"][field] = value
    manifest_path.write_text(json.dumps(manifest), encoding="utf-8")
    _reseal_scientific_fixture(root)

    problems = pact._scientific_extension_set_seal_problems(root, extension_set)

    assert any(problem in item for item in problems), problems


@pytest.mark.parametrize(
    ("field", "value", "problem"),
    [
        ("schema_version", 1, "schema_version must be 2"),
        ("kind", "legacy-set", "kind must be 'molt-source-extension-set'"),
        ("source_head", "stale", "source_head must be"),
        ("target_triple", "host", "target_triple must be 'wasm32-wasip1'"),
    ],
)
def test_proof_queue_rejects_scipy_set_manifest_identity_drift(
    tmp_path: Path, field: str, value: object, problem: str
) -> None:
    root = tmp_path / "pact_scipy_witness"
    _write_current_scientific_seal(root)
    set_manifest_path = root / "files" / "extension_set_manifest.json"
    manifest = json.loads(set_manifest_path.read_text(encoding="utf-8"))
    manifest[field] = value
    set_manifest_path.write_text(json.dumps(manifest), encoding="utf-8")
    _reseal_scientific_fixture(root)

    problems = pact._scientific_extension_set_seal_problems(
        root, scientific_extension_set("scipy", "pact-witness")
    )

    assert any(problem in item for item in problems), problems


@pytest.mark.parametrize(
    "mutation",
    [
        "ordered_set",
        "artifact_checksum",
        "wheel_checksum",
        "closure_checksum",
        "meson_digest",
        "meson_metadata_bytes",
        "config_tool_cross",
        "config_tool_set",
        "config_tools_invalid",
        "pkg_config_requirement",
        "installed_python",
        "installed_python_on_disk",
        "installed_declared_on_disk",
        "installed_path_escape",
        "installed_unexpected_file",
        "wheel_bytes",
        "wheel_missing",
        "top_level_extra",
        "extension_extra",
        "submodule_invalid",
        "backend_sha256",
        "target_tool_sha256",
        "build_requirements",
        "build_marker_environment",
        "build_active_requirements",
        "build_resolved",
        "build_resolved_shape",
        "build_resolved_order",
        "build_resolved_missing",
        "build_resolved_distribution",
        "build_resolved_version",
        "build_custody_address",
        "build_custody_python_sha256",
        "build_custody_uv_sha256",
        "missing_manifest",
    ],
)
def test_proof_queue_rejects_scipy_set_manifest_transaction_drift(
    tmp_path: Path, mutation: str
) -> None:
    root = tmp_path / "pact_scipy_witness"
    _write_current_scientific_seal(root)
    set_manifest_path = root / "files" / "extension_set_manifest.json"
    manifest = json.loads(set_manifest_path.read_text(encoding="utf-8"))
    if mutation == "ordered_set":
        manifest["extensions"] = list(reversed(manifest["extensions"]))
        expected = "ordered typed extension contract drift"
    elif mutation == "artifact_checksum":
        manifest["extensions"][0]["artifact_sha256"] = "0" * 64
        expected = "artifact_sha256 differs from sidecar"
    elif mutation == "wheel_checksum":
        manifest["extensions"][0]["wheel_sha256"] = "0" * 64
        expected = "wheel_sha256 differs from sidecar"
    elif mutation == "closure_checksum":
        manifest["extensions"][0]["object_closure_sha256"] = "0" * 64
        expected = "object_closure_sha256 differs from sidecar"
    elif mutation == "meson_digest":
        manifest["meson"]["intro_targets_sha256"] = ""
        expected = "meson.intro_targets_sha256 is not a SHA-256 digest"
    elif mutation == "meson_metadata_bytes":
        (root / "files/provenance/metadata/meson/intro-targets.json").write_text(
            '[{"drift":true}]\n', encoding="utf-8"
        )
        expected = "meson.intro_targets_sha256 mismatch"
    elif mutation == "config_tool_cross":
        manifest["meson"]["config_tool_cross_sha256"] = ""
        expected = "meson.config_tool_cross_sha256 is not a SHA-256 digest"
    elif mutation == "config_tool_set":
        manifest["meson"]["config_tools"].pop()
        expected = "Meson config tool set/order drift"
    elif mutation == "config_tools_invalid":
        manifest["meson"]["config_tools"] = None
        expected = "Meson config_tools are invalid"
    elif mutation == "pkg_config_requirement":
        manifest["meson"]["pkg_config_requirement"] = "pkgconf==0"
        expected = "Meson pkg-config requirement drift"
    elif mutation == "installed_python":
        manifest["installed_package_files"].remove("scipy/__config__.py")
        expected = "missing installed package files: scipy/__config__.py"
    elif mutation == "installed_python_on_disk":
        (root / "files/scipy/__config__.py").unlink()
        expected = "installed package files absent on disk: scipy/__config__.py"
    elif mutation == "installed_declared_on_disk":
        (root / "files/scipy/_fixture_extra.py").unlink()
        expected = "installed package files absent on disk: scipy/_fixture_extra.py"
    elif mutation == "installed_path_escape":
        manifest["installed_package_files"][0] = "../escape.py"
        manifest["installed_package_files"].sort()
        expected = "installed_package_files contain non-canonical paths"
    elif mutation == "installed_unexpected_file":
        unexpected = root / "files/scipy/rogue.py"
        unexpected.parent.mkdir(parents=True, exist_ok=True)
        unexpected.write_text("# rogue\n", encoding="utf-8")
        expected = "undeclared installed package files"
    elif mutation in {"wheel_bytes", "wheel_missing"}:
        extension = scientific_extension_set("scipy", "pact-witness").extensions[0]
        sidecar_path = pact._scientific_extension_manifest_path(
            root / "files", extension.module, extension.target
        )
        sidecar = json.loads(sidecar_path.read_text(encoding="utf-8"))
        wheel_path = (sidecar_path.parent / sidecar["wheel"]).resolve()
        if mutation == "wheel_bytes":
            wheel_path.write_bytes(b"drifted wheel")
        else:
            wheel_path.unlink()
        expected = "wheel is not sealed or checksummed"
    elif mutation == "top_level_extra":
        manifest["legacy"] = True
        expected = "top-level shape is invalid"
    elif mutation == "extension_extra":
        manifest["extensions"][0]["legacy"] = True
        expected = "extension shape is invalid"
    elif mutation == "submodule_invalid":
        manifest["submodules"] = [{"path": "../escape", "commit": "0" * 40}]
        expected = "submodule path is invalid"
    elif mutation == "backend_sha256":
        manifest["meson"]["backend"]["sha256"] = "z" * 64
        expected = "backend sha256 is invalid"
    elif mutation == "target_tool_sha256":
        manifest["target_metadata"]["toolchain"]["tools"]["cc"]["sha256"] = "z" * 64
        expected = "target c identity is invalid"
    elif mutation == "build_requirements":
        manifest["build_environment"]["requirements"] = []
        expected = "build requirements are invalid"
    elif mutation == "build_marker_environment":
        del manifest["build_environment"]["marker_environment"]["sys_platform"]
        expected = "marker environment is invalid"
    elif mutation == "build_active_requirements":
        manifest["build_environment"]["active_requirements"].pop()
        expected = "active requirements do not match"
    elif mutation == "build_resolved":
        manifest["build_environment"]["resolved"] = []
        expected = "resolved requirements are empty"
    elif mutation == "build_resolved_shape":
        manifest["build_environment"]["resolved"][0]["extra"] = "drift"
        expected = "resolved requirement shape is invalid"
    elif mutation == "build_resolved_order":
        manifest["build_environment"]["resolved"].reverse()
        expected = "resolved requirements are out of source order"
    elif mutation == "build_resolved_missing":
        manifest["build_environment"]["resolved"].pop()
        expected = "do not exactly cover the source requirement authority"
    elif mutation == "build_resolved_distribution":
        manifest["build_environment"]["resolved"][0]["distribution"] = "wrong"
        expected = "resolved distribution does not satisfy"
    elif mutation == "build_resolved_version":
        manifest["build_environment"]["resolved"][0]["version"] = "0"
        expected = "resolved version does not satisfy"
    elif mutation == "build_custody_address":
        manifest["build_environment"]["custody"]["environment_id"] = "0" * 64
        expected = "build-environment address digest is invalid"
    elif mutation == "build_custody_python_sha256":
        manifest["build_environment"]["custody"]["python"]["base_executable_sha256"] = (
            "z" * 64
        )
        expected = "build Python custody is invalid"
    elif mutation == "build_custody_uv_sha256":
        manifest["build_environment"]["custody"]["uv"]["sha256"] = "z" * 64
        expected = "uv custody is invalid"
    else:
        set_manifest_path.unlink()
        expected = "missing or unreadable extension-set manifest"
    if mutation != "missing_manifest":
        set_manifest_path.write_text(json.dumps(manifest), encoding="utf-8")
    _reseal_scientific_fixture(root)

    problems = pact._scientific_extension_set_seal_problems(
        root, scientific_extension_set("scipy", "pact-witness")
    )

    assert any(expected in item for item in problems), problems


def test_proof_queue_pact_witness_roots_accept_artifact_specific_manifests(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    durable_numpy = (
        tmp_path
        / "artifacts/package-seals/numpy/2.5.1/pact_numpy_multiarray_sealed_for_witness"
    )
    numpy_identity = _write_current_scientific_seal(durable_numpy, package="numpy")
    monkeypatch.setenv("MOLT_EXT_ROOT", str(tmp_path / "artifacts"))
    artifact_root = tmp_path / "artifacts/package-seals/scipy/1.18.0/pact_scipy_witness"
    scipy_identity = _write_current_scientific_seal(artifact_root)
    assert numpy_identity is not None and scipy_identity is not None
    _patch_pact_expected_identities(
        monkeypatch,
        {"numpy": numpy_identity, "scipy": scipy_identity},
        artifact_root=tmp_path / "artifacts",
    )

    roots = pact._pact_witness_native_roots(repo_root=tmp_path)

    assert roots == [
        (durable_numpy / "files").resolve(),
        (artifact_root / "files").resolve(),
    ]


def test_proof_queue_pact_witness_oracle_regenerates_parity_fixture() -> None:
    spec = pact._pact_witness_oracle_spec()

    assert spec["logical_id"] == "pact-witness-oracle-parity"
    assert spec["resource_family"] == "wasm-browser"
    assert spec["contention_key"] == "wasm:pact-witness"
    command = list(spec["command"])
    assert command[:7] == [
        "uv",
        "run",
        "--active",
        "--project",
        ".",
        "--python",
        "3.12",
    ]
    assert "--with" in command
    stack = resolve_scientific_stack()
    assert stack.numpy_requirement in command
    assert stack.scipy_requirement in command
    assert command[-2:] == ["python", "tools/pact_witness_oracle.py"]
    assert "collab/pact/pact_witness_kernel/make_fixture.py" in spec["scopes"]
    assert policy._proof_command_policy_error(command) is None


# ---------------------------------------------------------------------------
# Lifecycle / sqlite defects (PID reuse, lock stranding, contention TOCTOU).
# ---------------------------------------------------------------------------


def _insert_running_row(
    db: Path,
    tmp_path: Path,
    *,
    run_id: str = "active-run",
    contention_key: str = "python:lifecycle",
    guard_pid: int,
    started_at: str | None = None,
    guard_identity: object = "__auto__",
) -> None:
    """Insert a row already in the 'running' state with a guard PID.

    ``guard_identity`` defaults to whatever ``_update_run`` captures for the
    live ``guard_pid``; pass an explicit value (including None) to override.
    """
    conn = state._connect(db)
    scheduling._insert_run(
        conn,
        run_id=run_id,
        logical_id="active",
        reason="lifecycle regression",
        command=[sys.executable, "-c", "print('active')"],
        cwd=state.ROOT,
        resource_family="python",
        contention_key=contention_key,
        scopes=["tools/proof_queue.py"],
        log_path=tmp_path / f"{run_id}.log",
        summary_json=tmp_path / f"{run_id}.memory_guard.json",
    )
    state._update_run(
        conn,
        run_id,
        status="running",
        started_at=started_at or state._utc_now(),
        guard_pid=guard_pid,
    )
    if guard_identity != "__auto__":
        state._update_run(conn, run_id, guard_identity=guard_identity)


def test_prune_stale_reclaims_running_row_when_guard_pid_reused(
    monkeypatch: pytest.MonkeyPatch,
    tmp_path: Path,
    capsys: pytest.CaptureFixture[str],
) -> None:
    """Defect 1: a detached runner set status='running',guard_pid=P then died.

    Windows recycled P to an unrelated live process. Bare ``_pid_alive(P)`` is
    True, so pre-fix prune-stale left the row stuck 'running' forever. The
    identity recorded at launch no longer matches the live PID, so the reused
    PID must not count as a live guard and the row must be reclaimed.
    """
    db = tmp_path / "proof_queue.sqlite3"
    live_pid = os.getpid()
    monkeypatch.setattr(custody, "_pid_alive", lambda pid: int(pid) == live_pid)
    monkeypatch.setattr(
        custody,
        "_process_identity",
        lambda pid: f"{os.name}:{int(pid)}:live" if int(pid) == live_pid else None,
    )
    # Record an identity for the *original* (now-dead) guard that does not match
    # the live process now owning that PID. This is exactly the state a launch
    # write leaves after PID reuse.
    _insert_running_row(
        db,
        tmp_path,
        guard_pid=live_pid,
        guard_identity=f"{os.name}:{live_pid}:dead",
    )
    # The PID is genuinely alive (it is our own process); only identity differs.
    assert custody._pid_alive(live_pid)

    assert (
        cli.main(
            [
                "--db",
                str(db),
                "--logs-root",
                str(tmp_path / "runs"),
                "--repo-root",
                str(state.ROOT),
                "prune-stale",
            ]
        )
        == 0
    )

    out = capsys.readouterr().out
    assert "pruned=1" in out
    assert "reused-guard-pid" in out
    row = _rows(db)[0]
    assert row["status"] == "stale"
    assert row["returncode"] == custody.PROOF_QUEUE_STALE_EXIT_CODE


def test_prune_stale_reclaims_running_row_past_age_ceiling(
    tmp_path: Path,
    capsys: pytest.CaptureFixture[str],
) -> None:
    """Defect 1 (age ceiling): a live guard PID must not pin a row forever.

    A row whose guard PID is alive but that has been 'running' longer than the
    wall-clock ceiling is reclaimed regardless of liveness. Pre-fix prune-stale
    kept any row with a live guard, so a runner that wrote status='running' but
    never a terminal row stayed stuck indefinitely.
    """
    db = tmp_path / "proof_queue.sqlite3"
    live_pid = os.getpid()
    _dt = state.dt
    ancient_dt = _dt.datetime.now(_dt.UTC).replace(microsecond=0) - _dt.timedelta(
        seconds=custody.PROOF_QUEUE_RUNNING_AGE_CEILING_SECONDS + 60.0
    )
    started_at = ancient_dt.isoformat()
    # Identity matches the live PID, so this is NOT a reuse case: the only
    # reason to reclaim is the age ceiling.
    _insert_running_row(
        db,
        tmp_path,
        guard_pid=live_pid,
        started_at=started_at,
        guard_identity=custody._process_identity(live_pid),
    )
    assert custody._guard_process_live(live_pid, custody._process_identity(live_pid))

    assert (
        cli.main(
            [
                "--db",
                str(db),
                "--logs-root",
                str(tmp_path / "runs"),
                "--repo-root",
                str(state.ROOT),
                "prune-stale",
            ]
        )
        == 0
    )

    out = capsys.readouterr().out
    assert "pruned=1" in out
    assert "running-age-ceiling" in out
    row = _rows(db)[0]
    assert row["status"] == "stale"


def test_prune_stale_keeps_fresh_running_row_with_matching_identity(
    tmp_path: Path,
    capsys: pytest.CaptureFixture[str],
) -> None:
    """Guard: a genuinely live, recent guard with matching identity survives."""
    db = tmp_path / "proof_queue.sqlite3"
    live_pid = os.getpid()
    _insert_running_row(
        db,
        tmp_path,
        guard_pid=live_pid,
        guard_identity=custody._process_identity(live_pid),
    )

    assert (
        cli.main(
            [
                "--db",
                str(db),
                "--logs-root",
                str(tmp_path / "runs"),
                "--repo-root",
                str(state.ROOT),
                "prune-stale",
            ]
        )
        == 0
    )

    out = capsys.readouterr().out
    assert "pruned=0" in out
    assert _rows(db)[0]["status"] == "running"


class _LockingCommitConnection:
    """A thin ``sqlite3.Connection`` proxy whose ``commit`` fails on a lock.

    ``sqlite3.Connection.commit`` is a C method of an immutable type and cannot
    be monkeypatched, so we wrap a real connection and control only ``commit``.
    ``_update_run`` reaches the DB solely through ``execute`` (forwarded) and
    ``commit`` (throttled here), which is exactly the surface under test.
    """

    def __init__(self, conn: sqlite3.Connection, fail_times: int) -> None:
        self._conn = conn
        self._remaining = fail_times
        self.commit_calls = 0

    def commit(self) -> None:
        self.commit_calls += 1
        if self._remaining > 0:
            self._remaining -= 1
            raise sqlite3.OperationalError("database is locked")
        self._conn.commit()

    @property
    def remaining(self) -> int:
        return self._remaining

    def __getattr__(self, name: str) -> object:
        return getattr(self._conn, name)


def test_update_run_retries_terminal_write_when_database_locked(
    tmp_path: Path,
) -> None:
    """Defect 2: the terminal status write must not lose to a transient lock.

    Pre-fix ``_update_run`` did a single ``conn.commit()``; a concurrent writer
    holding the WAL write lock past the (unset) busy timeout raised
    'database is locked', which propagated out of the terminal status write and
    left the row stuck 'running' with the result lost. The write must retry the
    locked commit and then persist the terminal status.
    """
    db = tmp_path / "proof_queue.sqlite3"
    conn = state._connect(db)
    scheduling._insert_run(
        conn,
        run_id="lock-run",
        logical_id="active",
        reason="lock regression",
        command=[sys.executable, "-c", "print('active')"],
        cwd=state.ROOT,
        resource_family="python",
        contention_key="python:lock",
        scopes=["tools/proof_queue.py"],
        log_path=tmp_path / "lock.log",
        summary_json=tmp_path / "lock.memory_guard.json",
    )
    state._update_run(conn, "lock-run", status="running", started_at=state._utc_now())

    # A writer that holds the lock for the first 3 commit attempts, then
    # releases it. Without retry the first locked commit strands the row.
    flaky = _LockingCommitConnection(conn, fail_times=3)
    state._update_run(
        flaky,  # type: ignore[arg-type]
        "lock-run",
        status="passed",
        returncode=0,
        finished_at=state._utc_now(),
        elapsed_s=1.0,
    )

    assert flaky.remaining == 0  # all simulated locked attempts were consumed
    assert flaky.commit_calls == 4  # 3 locked + 1 success
    row = _rows(db)[0]
    assert row["status"] == "passed"
    assert row["returncode"] == 0


def test_update_run_reraises_persistent_database_lock(
    monkeypatch: pytest.MonkeyPatch,
    tmp_path: Path,
) -> None:
    """A lock that never clears still surfaces rather than silently vanishing."""
    db = tmp_path / "proof_queue.sqlite3"
    conn = state._connect(db)
    scheduling._insert_run(
        conn,
        run_id="stuck-run",
        logical_id="active",
        reason="persistent lock",
        command=[sys.executable, "-c", "print('active')"],
        cwd=state.ROOT,
        resource_family="python",
        contention_key="python:stuck",
        scopes=["tools/proof_queue.py"],
        log_path=tmp_path / "stuck.log",
        summary_json=tmp_path / "stuck.memory_guard.json",
    )
    monkeypatch.setattr(
        state,
        "PROOF_QUEUE_LOCKED_WRITE_RETRY_SLEEP_SECONDS",
        0.0,
    )
    # fail_times far larger than the retry budget => the lock never clears.
    flaky = _LockingCommitConnection(conn, fail_times=10_000)
    with pytest.raises(sqlite3.OperationalError, match="database is locked"):
        state._update_run(
            flaky,  # type: ignore[arg-type]
            "stuck-run",
            status="running",
        )
    # Exactly the bounded retry budget was attempted, then it surfaced.
    assert flaky.commit_calls == state.PROOF_QUEUE_LOCKED_WRITE_RETRIES


def test_connect_sets_busy_timeout(tmp_path: Path) -> None:
    """Defect 2: every connection must set a non-default busy timeout."""
    db = tmp_path / "proof_queue.sqlite3"
    conn = state._connect(db)
    (value,) = conn.execute("PRAGMA busy_timeout").fetchone()
    assert value == state.PROOF_QUEUE_SQLITE_BUSY_TIMEOUT_MS
    assert value >= 30_000


def test_contention_key_admits_only_one_running_run(tmp_path: Path) -> None:
    """Defect 3: two runs cannot both be 'running' for one contention key.

    The partial-unique index makes SQLite enforce at-most-one RUNNING run per
    contention key even when two admissions interleave their check and the
    status='running' transition. Pre-fix the plain writes let both rows reach
    'running', so two heavy builds would contend for the resource the key
    exists to serialize. Multiple QUEUED rows per key stay legal (dependency
    chains), so only the running->running collision is rejected.
    """
    db = tmp_path / "proof_queue.sqlite3"
    conn = state._connect(db)
    for run_id in ("first-active", "second-active"):
        scheduling._insert_run(
            conn,
            run_id=run_id,
            logical_id=run_id,
            reason="admission",
            command=[sys.executable, "-c", "print('x')"],
            cwd=state.ROOT,
            resource_family="rust",
            contention_key="cargo:shared-target",
            scopes=[],
            log_path=tmp_path / f"{run_id}.log",
            summary_json=tmp_path / f"{run_id}.memory_guard.json",
        )
    # Two QUEUED rows for the same key are allowed (dependency-chain pattern).
    queued = [r for r in _rows(db) if r["status"] == "queued"]
    assert len(queued) == 2

    # The first run transitions to 'running'.
    state._update_run(
        conn, "first-active", status="running", started_at=state._utc_now()
    )
    # A second concurrent transition to 'running' for the same key must be
    # rejected by the DB itself, independent of any application-level check.
    with pytest.raises(sqlite3.IntegrityError):
        state._update_run(
            conn, "second-active", status="running", started_at=state._utc_now()
        )

    running = [r for r in _rows(db) if r["status"] == "running"]
    assert len(running) == 1
    assert running[0]["run_id"] == "first-active"


def test_compiler_build_resource_mutex_admits_only_one_launched_run(
    tmp_path: Path,
) -> None:
    """Different heavy contention keys still share one compiler-build slot."""
    db = tmp_path / "proof_queue.sqlite3"
    conn = state._connect(db)
    for run_id, resource_family, contention_key in (
        ("wasm-active", "wasm-browser", "wasm:pact-witness"),
        ("native-active", "native-build", "native:runtime-build"),
    ):
        scheduling._insert_run(
            conn,
            run_id=run_id,
            logical_id=run_id,
            reason="compiler build admission",
            command=[sys.executable, "-c", "print('x')"],
            cwd=state.ROOT,
            resource_family=resource_family,
            contention_key=contention_key,
            scopes=[],
            log_path=tmp_path / f"{run_id}.log",
            summary_json=tmp_path / f"{run_id}.memory_guard.json",
        )

    rows = {row["run_id"]: row for row in _rows(db)}
    assert rows["wasm-active"]["resource_mutex_key"] == "compiler-build-resource"
    assert rows["native-active"]["resource_mutex_key"] == "compiler-build-resource"

    state._update_run(
        conn, "wasm-active", status="running", started_at=state._utc_now()
    )
    with pytest.raises(sqlite3.IntegrityError):
        state._update_run(
            conn,
            "native-active",
            status="dispatched",
            started_at=state._utc_now(),
        )

    active = [row for row in _rows(db) if row["status"] in {"dispatched", "running"}]
    assert [row["run_id"] for row in active] == ["wasm-active"]


def test_concurrent_detached_claims_serialize_one_compiler_build_lease(
    tmp_path: Path,
) -> None:
    """Independent schedulers race through SQLite; exactly one owns custody."""
    db = tmp_path / "proof_queue.sqlite3"
    seed = state._connect(db)
    for run_id, resource_family, contention_key in (
        ("native-racer", "native-build", "native:race"),
        ("wasm-racer", "wasm-browser", "wasm:race"),
    ):
        scheduling._insert_run(
            seed,
            run_id=run_id,
            logical_id=run_id,
            reason="concurrent compiler lease proof",
            command=[sys.executable, "-c", "print('race')"],
            cwd=state.ROOT,
            resource_family=resource_family,
            contention_key=contention_key,
            scopes=[],
            log_path=tmp_path / f"{run_id}.log",
            summary_json=tmp_path / f"{run_id}.memory_guard.json",
        )
    seed.close()

    start = Barrier(2)

    def claim(run_id: str) -> tuple[str, bool, str | None]:
        conn = state._connect(db)
        try:
            start.wait(timeout=5.0)
            row, reason = runner._claim_detached_run(conn, run_id, queue_size=2)
            return run_id, row is not None, reason
        finally:
            conn.close()

    with ThreadPoolExecutor(max_workers=2) as pool:
        results = list(pool.map(claim, ("native-racer", "wasm-racer")))

    assert sum(claimed for _, claimed, _ in results) == 1
    loser_reason = next(reason for _, claimed, reason in results if not claimed)
    assert loser_reason is not None
    assert "resource mutex 'compiler-build-resource'" in loser_reason
    statuses = {row["run_id"]: row["status"] for row in _rows(db)}
    assert sorted(statuses.values()) == ["dispatched", "queued"]


def test_admit_run_allows_multiple_queued_rows_per_contention_key(
    tmp_path: Path,
) -> None:
    """Queued rows are wait-list state; launch claim owns resource custody."""
    db = tmp_path / "proof_queue.sqlite3"
    conn = state._connect(db)
    assert (
        scheduling._admit_run(
            conn,
            run_id="gate-first",
            logical_id="first",
            reason="first",
            command=[sys.executable, "-c", "print('first')"],
            cwd=state.ROOT,
            resource_family="rust",
            contention_key="cargo:gate",
            scopes=[],
            log_path=tmp_path / "gate-first.log",
            summary_json=tmp_path / "gate-first.memory_guard.json",
        )
        is None
    )
    assert (
        scheduling._admit_run(
            conn,
            run_id="gate-second",
            logical_id="second",
            reason="second",
            command=[sys.executable, "-c", "print('second')"],
            cwd=state.ROOT,
            resource_family="rust",
            contention_key="cargo:gate",
            scopes=[],
            log_path=tmp_path / "gate-second.log",
            summary_json=tmp_path / "gate-second.memory_guard.json",
        )
        is None
    )

    rows = _rows(db)
    assert [r["run_id"] for r in rows] == ["gate-first", "gate-second"]
    assert {r["status"] for r in rows} == {"queued"}


def test_proof_queue_audit_treats_queued_rows_as_wait_list(
    tmp_path: Path, capsys: pytest.CaptureFixture[str]
) -> None:
    db = tmp_path / "proof_queue.sqlite3"
    conn = state._connect(db)
    for run_id, status in (
        ("gate-first", "queued"),
        ("gate-second", "queued"),
        ("gate-running", "dispatched"),
    ):
        log_path = tmp_path / f"{run_id}.log"
        log_path.write_text("queued wait-list row\n", encoding="utf-8")
        scheduling._insert_run(
            conn,
            run_id=run_id,
            logical_id=run_id,
            reason="prove queued rows do not own active contention custody",
            command=[sys.executable, "-c", "print('proof')"],
            cwd=state.ROOT,
            resource_family="wasm-browser",
            contention_key="wasm:pact-witness",
            scopes=["tools/proof_queue.py"],
            git_snapshot={
                "available": True,
                "head": "abc123",
                "dirty": False,
                "status": [],
            },
            log_path=log_path,
            summary_json=tmp_path / f"{run_id}.memory_guard.json",
        )
        state._insert_note(
            conn,
            run_id=run_id,
            body="test: queued rows are wait-list state, not resource custody",
            kind="submission",
            author="codex",
        )
        if status != "queued":
            state._update_run(conn, run_id, status=status, started_at=state._utc_now())

    assert (
        cli.main(
            [
                "--db",
                str(db),
                "--logs-root",
                str(tmp_path / "runs"),
                "--repo-root",
                str(state.ROOT),
                "audit",
                "--no-notebook-check",
            ]
        )
        == 0
    )
    output = capsys.readouterr().out
    assert "active=1" in output
    assert "queue-contention-duplicate" not in output


def test_detached_claim_serializes_contention_key(tmp_path: Path) -> None:
    db = tmp_path / "proof_queue.sqlite3"
    conn = state._connect(db)
    scheduling._admit_run(
        conn,
        run_id="gate-first",
        logical_id="first",
        reason="first",
        command=[sys.executable, "-c", "print('first')"],
        cwd=state.ROOT,
        resource_family="rust",
        contention_key="cargo:gate",
        scopes=[],
        log_path=tmp_path / "gate-first.log",
        summary_json=tmp_path / "gate-first.memory_guard.json",
    )
    scheduling._admit_run(
        conn,
        run_id="gate-second",
        logical_id="second",
        reason="second",
        command=[sys.executable, "-c", "print('second')"],
        cwd=state.ROOT,
        resource_family="rust",
        contention_key="cargo:gate",
        scopes=[],
        log_path=tmp_path / "gate-second.log",
        summary_json=tmp_path / "gate-second.memory_guard.json",
    )

    claimed, reason = runner._claim_detached_run(conn, "gate-first", queue_size=2)
    assert claimed is not None
    assert reason is None
    blocked, reason = runner._claim_detached_run(conn, "gate-second", queue_size=2)
    assert blocked is None
    assert reason is not None
    assert "contention key 'cargo:gate' already has active run(s)" in reason
    assert "gate-first" in reason
    rows = {r["run_id"]: r for r in _rows(db)}
    assert rows["gate-first"]["status"] == "dispatched"
    assert rows["gate-second"]["status"] == "queued"


def test_detached_claim_serializes_compiler_build_resource_mutex(
    tmp_path: Path,
) -> None:
    db = tmp_path / "proof_queue.sqlite3"
    conn = state._connect(db)
    scheduling._admit_run(
        conn,
        run_id="native-build-first",
        logical_id="native",
        reason="native build",
        command=[sys.executable, "-c", "print('native')"],
        cwd=state.ROOT,
        resource_family="native-build",
        contention_key="native:molt-run",
        scopes=[],
        log_path=tmp_path / "native.log",
        summary_json=tmp_path / "native.memory_guard.json",
    )
    scheduling._admit_run(
        conn,
        run_id="wasm-browser-second",
        logical_id="wasm",
        reason="wasm browser build",
        command=[sys.executable, "-c", "print('wasm')"],
        cwd=state.ROOT,
        resource_family="wasm-browser",
        contention_key="wasm:pact",
        scopes=[],
        log_path=tmp_path / "wasm.log",
        summary_json=tmp_path / "wasm.memory_guard.json",
    )
    scheduling._admit_run(
        conn,
        run_id="python-third",
        logical_id="python",
        reason="light python proof",
        command=[sys.executable, "-c", "print('python')"],
        cwd=state.ROOT,
        resource_family="python",
        contention_key="python:light",
        scopes=[],
        log_path=tmp_path / "python.log",
        summary_json=tmp_path / "python.memory_guard.json",
    )

    claimed, reason = runner._claim_detached_run(
        conn, "native-build-first", queue_size=3
    )
    assert claimed is not None
    assert reason is None

    blocked, reason = runner._claim_detached_run(
        conn, "wasm-browser-second", queue_size=3
    )
    assert blocked is None
    assert reason is not None
    assert "resource mutex 'compiler-build-resource'" in reason

    light, reason = runner._claim_detached_run(conn, "python-third", queue_size=3)
    assert light is not None
    assert reason is None

    rows = {r["run_id"]: r for r in _rows(db)}
    assert rows["native-build-first"]["status"] == "dispatched"
    assert rows["wasm-browser-second"]["status"] == "queued"
    assert rows["python-third"]["status"] == "dispatched"


def test_queue_terminal_transition_frees_contention_key(tmp_path: Path) -> None:
    """The partial-unique index must not block the queued->running->done path.

    A single row moving queued -> running -> passed stays legal (same row), and
    once terminal a fresh admission for the same key succeeds.
    """
    db = tmp_path / "proof_queue.sqlite3"
    conn = state._connect(db)
    assert (
        scheduling._admit_run(
            conn,
            run_id="cycle-run",
            logical_id="cycle",
            reason="cycle",
            command=[sys.executable, "-c", "print('cycle')"],
            cwd=state.ROOT,
            resource_family="python",
            contention_key="python:cycle",
            scopes=[],
            log_path=tmp_path / "cycle.log",
            summary_json=tmp_path / "cycle.memory_guard.json",
        )
        is None
    )
    # queued -> running (same row, still non-terminal) must be allowed.
    state._update_run(conn, "cycle-run", status="running", started_at=state._utc_now())
    # running -> passed (terminal) frees the key.
    state._update_run(
        conn,
        "cycle-run",
        status="passed",
        returncode=0,
        finished_at=state._utc_now(),
    )
    # A fresh admission for the same key now succeeds.
    assert (
        scheduling._admit_run(
            conn,
            run_id="cycle-run-2",
            logical_id="cycle",
            reason="cycle again",
            command=[sys.executable, "-c", "print('cycle2')"],
            cwd=state.ROOT,
            resource_family="python",
            contention_key="python:cycle",
            scopes=[],
            log_path=tmp_path / "cycle2.log",
            summary_json=tmp_path / "cycle2.memory_guard.json",
        )
        is None
    )
