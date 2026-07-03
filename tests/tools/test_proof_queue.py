from __future__ import annotations

import json
import os
import sqlite3
import subprocess
import sys
import time
from pathlib import Path
from types import SimpleNamespace

import pytest

import tools.proof_queue as proof_queue

_TEST_GIT_SNAPSHOT = {
    "available": True,
    "head": "test-head",
    "dirty": False,
    "status": [],
    "ignored_status_count": 0,
}
_REAL_GIT_SNAPSHOT_TESTS = {
    "test_proof_queue_git_snapshot_ignores_generated_wasm_checksums",
}


@pytest.fixture(autouse=True)
def _proof_queue_unit_git_snapshot(
    request: pytest.FixtureRequest, monkeypatch: pytest.MonkeyPatch
) -> None:
    if request.node.name in _REAL_GIT_SNAPSHOT_TESTS:
        return
    monkeypatch.setattr(proof_queue, "_git_snapshot", lambda cwd: _TEST_GIT_SNAPSHOT)


def _rows(db: Path) -> list[sqlite3.Row]:
    conn = sqlite3.connect(db)
    conn.row_factory = sqlite3.Row
    return list(conn.execute("SELECT * FROM proof_runs ORDER BY rowid"))


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
    conn = proof_queue._connect(db)
    for run_id, status, key in (
        ("failed-parent", "failed", "python:failed-parent"),
        ("blocked-child", "queued", contention_key),
    ):
        proof_queue._insert_run(
            conn,
            run_id=run_id,
            logical_id=run_id,
            reason="prove blocked dependency reconciliation",
            command=[sys.executable, "-c", "print('blocked')"],
            cwd=proof_queue.ROOT,
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
            values["finished_at"] = proof_queue._utc_now()
        proof_queue._update_run(conn, run_id, **values)
    proof_queue._insert_edge(
        conn,
        parent_run_id="failed-parent",
        child_run_id="blocked-child",
        kind="depends_on",
        note="child waits on failed parent",
    )


def test_proof_queue_session_id_is_contention_key_scoped() -> None:
    assert proof_queue._proof_session_id(
        "wasm", "wasm-build"
    ) == proof_queue._proof_session_id("wasm", "wasm-build")
    assert proof_queue._proof_session_id(
        "wasm", "wasm-build"
    ) != proof_queue._proof_session_id("wasm", "wasm-browser")


def test_proof_queue_pid_alive_detects_current_process() -> None:
    assert proof_queue._pid_alive(os.getpid())
    assert not proof_queue._pid_alive(0)


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
    snapshot = proof_queue._git_snapshot(tmp_path)
    assert snapshot["dirty"] is False
    assert snapshot["status"] == []
    assert snapshot["ignored_status_count"] == 2

    (tmp_path / "src" / "app.py").write_text("print('changed')\n", encoding="utf-8")
    snapshot = proof_queue._git_snapshot(tmp_path)
    assert snapshot["dirty"] is True
    assert any("src/app.py" in line for line in snapshot["status"])


def test_proof_queue_exec_records_passed_run(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    db = tmp_path / "proof_queue.sqlite3"
    logs = tmp_path / "runs"
    notebooks = tmp_path / "notebooks"
    monkeypatch.setenv("MOLT_MEMORY_GUARD_POLL_SEC", "0.1")

    rc = proof_queue.main(
        [
            "--db",
            str(db),
            "--logs-root",
            str(logs),
            "--notebooks-root",
            str(notebooks),
            "--repo-root",
            str(proof_queue.ROOT),
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
        proof_queue.main(
            [
                "--db",
                str(db),
                "--logs-root",
                str(logs),
                "--repo-root",
                str(proof_queue.ROOT),
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
        proof_queue.main([subcommand, "--help"])

    assert exc.value.code == 0
    captured = capsys.readouterr()
    assert expected in captured.out
    assert "requires `--` before the proof command" not in captured.err


def test_proof_queue_help_detection_ignores_metadata_values_and_command_args() -> None:
    assert proof_queue._proof_command_help_requested(["exec", "--help"])
    assert proof_queue._proof_command_help_requested(["exec", "--id", "help-smoke", "-h"])
    assert not proof_queue._proof_command_help_requested(
        ["exec", "--note", "--help", "--", sys.executable]
    )
    assert not proof_queue._proof_command_help_requested(
        ["exec", "--note=--help", "--", sys.executable]
    )
    assert not proof_queue._proof_command_help_requested(["exec", "--", "--help"])


def test_proof_queue_exec_preserves_command_help_after_delimiter(
    tmp_path: Path, capsys: pytest.CaptureFixture[str]
) -> None:
    db = tmp_path / "proof_queue.sqlite3"
    logs = tmp_path / "runs"

    assert (
        proof_queue.main(
            [
                "--db",
                str(db),
                "--logs-root",
                str(logs),
                "--repo-root",
                str(proof_queue.ROOT),
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
        proof_queue.main(
            [
                "--db",
                str(db),
                "--logs-root",
                str(logs),
                "--repo-root",
                str(proof_queue.ROOT),
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
        proof_queue.main(
            [
                "--db",
                str(db),
                "--logs-root",
                str(logs),
                "--repo-root",
                str(proof_queue.ROOT),
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

    rc = proof_queue.main(
        [
            "--db",
            str(db),
            "--logs-root",
            str(logs),
            "--repo-root",
            str(proof_queue.ROOT),
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
        proof_queue._proof_queue_memory_guard_poll_sec(
            {"MOLT_MEMORY_GUARD_POLL_SEC": "not-a-number"}
        )
    with pytest.raises(ValueError, match="MOLT_MEMORY_GUARD_POLL_SEC"):
        proof_queue._proof_queue_memory_guard_poll_sec(
            {"MOLT_MEMORY_GUARD_POLL_SEC": "0"}
        )


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

    monkeypatch.setattr(proof_queue, "_launch_detached_runner", fake_launch)

    rc = proof_queue.main(
        [
            "--db",
            str(db),
            "--logs-root",
            str(logs),
            "--repo-root",
            str(proof_queue.ROOT),
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
        str(proof_queue.ROOT),
    ]
    assert (
        proof_queue.main(
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
    assert proof_queue.main([*base_args, "evidence", run_id]) == 0
    positional_payload = json.loads(capsys.readouterr().out)
    assert [item["run_id"] for item in positional_payload] == [run_id]

    assert proof_queue.main([*base_args, "evidence", "--run-id", run_id]) == 0
    flag_payload = json.loads(capsys.readouterr().out)
    assert [item["run_id"] for item in flag_payload] == [run_id]

    with pytest.raises(SystemExit, match="unknown proof run id"):
        proof_queue.main([*base_args, "evidence", "not-a-run-id"])

    with pytest.raises(SystemExit, match="positional and --run-id disagree"):
        proof_queue.main([*base_args, "evidence", run_id, "--run-id", "not-a-run-id"])


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

    monkeypatch.setattr(proof_queue, "_write_marimo_notebook", fail_notebook)

    rc = proof_queue.main(
        [
            "--db",
            str(db),
            "--logs-root",
            str(logs),
            "--repo-root",
            str(proof_queue.ROOT),
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
        proof_queue.main(
            [
                "--db",
                str(db),
                "--logs-root",
                str(logs),
                "--repo-root",
                str(proof_queue.ROOT),
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

    monkeypatch.setattr(proof_queue, "_insert_note", fail_insert_note)

    rc = proof_queue.main(
        [
            "--db",
            str(db),
            "--logs-root",
            str(logs),
            "--repo-root",
            str(proof_queue.ROOT),
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
        proof_queue.main(
            [
                "--db",
                str(db),
                "--logs-root",
                str(logs),
                "--repo-root",
                str(proof_queue.ROOT),
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

    rc = proof_queue.main(
        [
            "--db",
            str(db),
            "--logs-root",
            str(logs),
            "--repo-root",
            str(proof_queue.ROOT),
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
    conn = proof_queue._connect(db)
    proof_queue._insert_run(
        conn,
        run_id="active-run",
        logical_id="active",
        reason="already running",
        command=[sys.executable, "-c", "print('active')"],
        cwd=proof_queue.ROOT,
        resource_family="python",
        contention_key="python:shared",
        scopes=[],
        log_path=tmp_path / "active.log",
        summary_json=tmp_path / "active.memory_guard.json",
    )

    rc = proof_queue.main(
        [
            "--db",
            str(db),
            "--logs-root",
            str(tmp_path / "runs"),
            "--repo-root",
            str(proof_queue.ROOT),
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
    conn = proof_queue._connect(db)
    proof_queue._insert_run(
        conn,
        run_id="active-run",
        logical_id="active",
        reason="show active phase",
        command=[sys.executable, "-c", "print('active')"],
        cwd=proof_queue.ROOT,
        resource_family="wasm",
        contention_key="wasm-build",
        scopes=[],
        log_path=log_path,
        summary_json=tmp_path / "active.memory_guard.json",
    )
    proof_queue._update_run(
        conn, "active-run", status="running", started_at=proof_queue._utc_now()
    )

    assert (
        proof_queue.main(
            [
                "--db",
                str(db),
                "--logs-root",
                str(tmp_path / "runs"),
                "--repo-root",
                str(proof_queue.ROOT),
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
    conn = proof_queue._connect(db)
    proof_queue._insert_run(
        conn,
        run_id="active-run",
        logical_id="active",
        reason="show active pytest phase",
        command=[sys.executable, "-m", "pytest", "tests/test_molt_dev.py"],
        cwd=proof_queue.ROOT,
        resource_family="python-tests",
        contention_key="molt-dev",
        scopes=[],
        log_path=log_path,
        summary_json=summary_path,
    )
    proof_queue._update_run(
        conn, "active-run", status="running", started_at=proof_queue._utc_now()
    )

    assert (
        proof_queue.main(
            [
                "--db",
                str(db),
                "--logs-root",
                str(tmp_path / "runs"),
                "--repo-root",
                str(proof_queue.ROOT),
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
    conn = proof_queue._connect(db)
    proof_queue._insert_run(
        conn,
        run_id="active-run",
        logical_id="active",
        reason="show non-pytest rows do not inherit pytest custody noise",
        command=[sys.executable, "tests/molt_diff.py", "--jobs", "1"],
        cwd=proof_queue.ROOT,
        resource_family="python",
        contention_key="python:r6",
        scopes=[],
        log_path=log_path,
        summary_json=summary_path,
    )
    proof_queue._update_run(
        conn, "active-run", status="running", started_at=proof_queue._utc_now()
    )

    assert (
        proof_queue.main(
            [
                "--db",
                str(db),
                "--logs-root",
                str(tmp_path / "runs"),
                "--repo-root",
                str(proof_queue.ROOT),
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
        - proof_queue.RUNNING_PYTEST_CURRENT_TEST_MISSING_STALE_SECONDS
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
                        str(proof_queue.ROOT / "tools" / "memory_guard.py"),
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
    conn = proof_queue._connect(db)
    proof_queue._insert_run(
        conn,
        run_id="active-run",
        logical_id="active",
        reason="diagnose quiet pytest startup",
        command=[sys.executable, "-m", "pytest", "tests/test_molt_dev.py"],
        cwd=proof_queue.ROOT,
        resource_family="python-tests",
        contention_key="molt-dev",
        scopes=["tools/proof_queue.py"],
        log_path=log_path,
        summary_json=summary_path,
    )
    proof_queue._update_run(
        conn, "active-run", status="running", started_at=proof_queue._utc_now()
    )

    assert (
        proof_queue.main(
            [
                "--db",
                str(db),
                "--logs-root",
                str(tmp_path / "runs"),
                "--repo-root",
                str(proof_queue.ROOT),
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
        - proof_queue.RUNNING_PYTEST_CURRENT_TEST_MISSING_STALE_SECONDS
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
                        str(proof_queue.ROOT / "tools" / "memory_guard.py"),
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
    conn = proof_queue._connect(db)
    proof_queue._insert_run(
        conn,
        run_id="active-run",
        logical_id="active",
        reason="audit quiet pytest startup",
        command=[sys.executable, "-m", "pytest", "tests/test_molt_dev.py"],
        cwd=proof_queue.ROOT,
        resource_family="python-tests",
        contention_key="molt-dev",
        scopes=["tools/proof_queue.py"],
        log_path=log_path,
        summary_json=summary_path,
    )
    proof_queue._insert_note(
        conn,
        run_id="active-run",
        body="test: audit must surface current-test custody opacity",
        kind="submission",
        author="codex",
    )
    proof_queue._update_run(
        conn,
        "active-run",
        status="running",
        started_at=proof_queue._utc_now(),
        guard_pid=os.getpid(),
    )

    assert (
        proof_queue.main(
            [
                "--db",
                str(db),
                "--logs-root",
                str(tmp_path / "runs"),
                "--repo-root",
                str(proof_queue.ROOT),
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
        - proof_queue.RUNNING_PYTEST_CURRENT_TEST_MISSING_STALE_SECONDS
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
    conn = proof_queue._connect(db)
    proof_queue._insert_run(
        conn,
        run_id="active-run",
        logical_id="active",
        reason="diagnose pytest progress without current marker",
        command=[sys.executable, "-m", "pytest", "tests/tools/test_proof_queue.py"],
        cwd=proof_queue.ROOT,
        resource_family="python-tests",
        contention_key="proof-queue",
        scopes=["tools/proof_queue.py"],
        log_path=log_path,
        summary_json=summary_path,
    )
    proof_queue._update_run(
        conn, "active-run", status="running", started_at=proof_queue._utc_now()
    )

    assert (
        proof_queue.main(
            [
                "--db",
                str(db),
                "--logs-root",
                str(tmp_path / "runs"),
                "--repo-root",
                str(proof_queue.ROOT),
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
    log_path.write_text(f"proof_queue run_id=active-run\n{progress}\n", encoding="utf-8")
    stale = (
        time.time()
        - proof_queue.RUNNING_PYTEST_CURRENT_TEST_MISSING_STALE_SECONDS
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
    conn = proof_queue._connect(db)
    proof_queue._insert_run(
        conn,
        run_id="active-run",
        logical_id="active",
        reason="diagnose pytest failure progress before final report",
        command=[sys.executable, "-m", "pytest", "tests/tools/test_proof_queue.py"],
        cwd=proof_queue.ROOT,
        resource_family="python-tests",
        contention_key="proof-queue",
        scopes=["tools/proof_queue.py"],
        log_path=log_path,
        summary_json=summary_path,
    )
    proof_queue._update_run(
        conn, "active-run", status="running", started_at=proof_queue._utc_now()
    )

    assert (
        proof_queue.main(
            [
                "--db",
                str(db),
                "--logs-root",
                str(tmp_path / "runs"),
                "--repo-root",
                str(proof_queue.ROOT),
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
        proof_queue.main(
            [
                "--db",
                str(db),
                "--logs-root",
                str(tmp_path / "runs"),
                "--repo-root",
                str(proof_queue.ROOT),
                "diagnose",
                "active-run",
            ]
        )
        == 0
    )

    diagnose_out = capsys.readouterr().out
    assert "running-pytest-failures-observed" in diagnose_out
    assert "last_pytest_progress=..FFF.....FF......FF..FF................" in diagnose_out
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
    stale = time.time() - proof_queue.RUNNING_CHILD_MISSING_STALE_LOG_SECONDS - 5.0
    os.utime(log_path, (stale, stale))
    summary_path.write_text(
        json.dumps(
            {
                "status": "child_running",
                "child_process": {
                    "pid": child_pid,
                    "command": [
                        sys.executable,
                        str(proof_queue.ROOT / "tools" / "memory_guard.py"),
                    ],
                },
            }
        ),
        encoding="utf-8",
    )
    monkeypatch.setattr(
        proof_queue, "_pid_alive", lambda pid: pid in {child_pid, 99_001}
    )
    monkeypatch.setattr(memory_guard, "sample_processes", lambda: {})
    monkeypatch.setattr(memory_guard, "descendant_pids", lambda samples, pid: set())
    conn = proof_queue._connect(db)
    proof_queue._insert_run(
        conn,
        run_id="active-run",
        logical_id="active",
        reason="prove stale nested guard diagnosis",
        command=[sys.executable, "-c", "print('active')"],
        cwd=proof_queue.ROOT,
        resource_family="wasm",
        contention_key="wasm-build",
        scopes=["tools/proof_queue.py"],
        log_path=log_path,
        summary_json=summary_path,
    )
    proof_queue._update_run(
        conn,
        "active-run",
        status="running",
        guard_pid=99_001,
        started_at=proof_queue._utc_now(),
    )

    assert (
        proof_queue.main(
            [
                "--db",
                str(db),
                "--logs-root",
                str(tmp_path / "runs"),
                "--repo-root",
                str(proof_queue.ROOT),
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
    stale = time.time() - proof_queue.RUNNING_CHILD_MISSING_STALE_LOG_SECONDS - 5.0
    os.utime(log_path, (stale, stale))
    summary_path.write_text(
        json.dumps(
            {
                "status": "child_running",
                "child_process": {
                    "pid": child_pid,
                    "command": [
                        sys.executable,
                        str(proof_queue.ROOT / "tools" / "memory_guard.py"),
                    ],
                },
            }
        ),
        encoding="utf-8",
    )
    monkeypatch.setattr(proof_queue, "_pid_alive", lambda pid: pid == child_pid)
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
    conn = proof_queue._connect(db)
    proof_queue._insert_run(
        conn,
        run_id="active-run",
        logical_id="active",
        reason="prove stale live-child diagnosis",
        command=[sys.executable, "-c", "print('active')"],
        cwd=proof_queue.ROOT,
        resource_family="rust",
        contention_key="cargo-molt-runtime",
        scopes=["tools/proof_queue.py"],
        log_path=log_path,
        summary_json=summary_path,
    )
    proof_queue._update_run(
        conn,
        "active-run",
        status="running",
        guard_pid=99_001,
        started_at=proof_queue._utc_now(),
    )

    assert (
        proof_queue.main(
            [
                "--db",
                str(db),
                "--logs-root",
                str(tmp_path / "runs"),
                "--repo-root",
                str(proof_queue.ROOT),
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
    stale = time.time() - proof_queue.RUNNING_CHILD_MISSING_STALE_LOG_SECONDS - 5.0
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
    conn = proof_queue._connect(db)
    proof_queue._insert_run(
        conn,
        run_id="active-run",
        logical_id="active",
        reason="prove stale launch summary diagnosis",
        command=[sys.executable, "-c", "print('active')"],
        cwd=proof_queue.ROOT,
        resource_family="wasm",
        contention_key="wasm-build",
        scopes=["tools/proof_queue.py"],
        log_path=log_path,
        summary_json=summary_path,
    )
    proof_queue._update_run(
        conn,
        "active-run",
        status="running",
        guard_pid=99_001,
        started_at=proof_queue._utc_now(),
    )

    assert (
        proof_queue.main(
            [
                "--db",
                str(db),
                "--logs-root",
                str(tmp_path / "runs"),
                "--repo-root",
                str(proof_queue.ROOT),
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
                        str(proof_queue.ROOT / "tools" / "memory_guard.py"),
                    ],
                },
            }
        ),
        encoding="utf-8",
    )
    conn = proof_queue._connect(db)
    proof_queue._insert_run(
        conn,
        run_id="failed-run",
        logical_id="failed",
        reason="prove terminal unfinished guard summaries are classified",
        command=[sys.executable, "-c", "print('failed')"],
        cwd=proof_queue.ROOT,
        resource_family="python",
        contention_key="python-proof",
        scopes=["tools/proof_queue.py"],
        log_path=log_path,
        summary_json=summary_path,
    )
    proof_queue._insert_note(
        conn,
        run_id="failed-run",
        body="test: terminal memory_guard summary must not collapse to unclassified",
        kind="submission",
        author="codex",
    )
    proof_queue._update_run(
        conn,
        "failed-run",
        status="failed",
        returncode=15,
        elapsed_s=18.812,
    )

    assert (
        proof_queue.main(
            [
                "--db",
                str(db),
                "--logs-root",
                str(tmp_path / "runs"),
                "--repo-root",
                str(proof_queue.ROOT),
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
        proof_queue.main(
            [
                "--db",
                str(db),
                "--logs-root",
                str(tmp_path / "runs"),
                "--repo-root",
                str(proof_queue.ROOT),
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
    conn = proof_queue._connect(db)
    proof_queue._insert_run(
        conn,
        run_id="stale-run",
        logical_id="stale",
        reason="prove pruned stale rows remain visible without failing audit",
        command=[sys.executable, "-c", "print('stale')"],
        cwd=proof_queue.ROOT,
        resource_family="python",
        contention_key="python-proof",
        scopes=["tools/proof_queue.py"],
        log_path=log_path,
        summary_json=summary_path,
    )
    proof_queue._insert_note(
        conn,
        run_id="stale-run",
        body="test: stale row intentionally pruned after custody loss",
        kind="finding",
        author="codex",
    )
    proof_queue._update_run(conn, "stale-run", status="stale")

    assert (
        proof_queue.main(
            [
                "--db",
                str(db),
                "--logs-root",
                str(tmp_path / "runs"),
                "--repo-root",
                str(proof_queue.ROOT),
                "audit",
                "--no-notebook-check",
            ]
        )
        == 0
    )
    out = capsys.readouterr().out
    assert "warning audit-memory-guard-summary-incomplete run=stale-run" in out
    assert "error audit-memory-guard-summary-incomplete run=stale-run" not in out


def test_proof_queue_prune_stale_uses_running_launch_summary_diagnosis(
    monkeypatch: pytest.MonkeyPatch,
    tmp_path: Path,
    capsys: pytest.CaptureFixture[str],
) -> None:
    db = tmp_path / "proof_queue.sqlite3"
    log_path = tmp_path / "active.log"
    summary_path = tmp_path / "active.memory_guard.json"
    log_path.write_text(
        "proof_queue run_id=active-run\n"
        " done\n",
        encoding="utf-8",
    )
    stale = time.time() - proof_queue.RUNNING_CHILD_MISSING_STALE_LOG_SECONDS - 5.0
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
    monkeypatch.setattr(proof_queue, "_pid_alive", lambda pid: pid == 99_001)
    conn = proof_queue._connect(db)
    proof_queue._insert_run(
        conn,
        run_id="active-run",
        logical_id="active",
        reason="prove prune follows stale diagnosis",
        command=[sys.executable, "-c", "print('active')"],
        cwd=proof_queue.ROOT,
        resource_family="wasm",
        contention_key="wasm-build",
        scopes=["tools/proof_queue.py"],
        log_path=log_path,
        summary_json=summary_path,
    )
    proof_queue._update_run(
        conn,
        "active-run",
        status="running",
        guard_pid=99_001,
        started_at=proof_queue._utc_now(),
    )

    assert (
        proof_queue.main(
            [
                "--db",
                str(db),
                "--logs-root",
                str(tmp_path / "runs"),
                "--repo-root",
                str(proof_queue.ROOT),
                "prune-stale",
            ]
        )
        == 0
    )

    out = capsys.readouterr().out
    assert "stale active-run" in out
    assert "diagnosis=running-proof-launch-summary-stale" in out
    assert str(summary_path) in out
    assert str(log_path) in out
    assert "pruned=1" in out
    assert _rows(db)[0]["status"] == "stale"


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
    stale = time.time() - proof_queue.RUNNING_CHILD_MISSING_STALE_LOG_SECONDS - 5.0
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
                        str(proof_queue.ROOT / "tools" / "memory_guard.py"),
                    ],
                },
            }
        ),
        encoding="utf-8",
    )
    monkeypatch.setattr(proof_queue, "_pid_alive", lambda pid: pid == guard_pid)
    conn = proof_queue._connect(db)
    proof_queue._insert_run(
        conn,
        run_id="active-run",
        logical_id="active",
        reason="prove prune terminalizes dead nested guard child",
        command=[sys.executable, "-c", "print('active')"],
        cwd=proof_queue.ROOT,
        resource_family="python-tests",
        contention_key="proof-queue-dx",
        scopes=["tools/proof_queue.py"],
        log_path=log_path,
        summary_json=summary_path,
    )
    proof_queue._update_run(
        conn,
        "active-run",
        status="running",
        guard_pid=guard_pid,
        started_at=proof_queue._utc_now(),
    )

    assert (
        proof_queue.main(
            [
                "--db",
                str(db),
                "--logs-root",
                str(tmp_path / "runs"),
                "--repo-root",
                str(proof_queue.ROOT),
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
    assert _rows(db)[0]["status"] == "stale"


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
                                str(proof_queue.ROOT / "tools" / "memory_guard.py"),
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

    monkeypatch.setattr(proof_queue.subprocess, "Popen", FakePopen)
    monkeypatch.setattr(
        proof_queue,
        "_git_snapshot",
        lambda cwd: {
            "available": True,
            "head": "abc123",
            "dirty": False,
            "status": [],
        },
    )
    monkeypatch.setattr(proof_queue, "_pid_alive", lambda pid: pid == guard_pid)
    monkeypatch.setattr(proof_queue, "PROOF_QUEUE_ACTIVE_POLL_SECONDS", 0.01)
    monkeypatch.setattr(
        proof_queue,
        "PROOF_QUEUE_STALE_TERMINATE_GRACE_SECONDS",
        0.01,
    )
    monkeypatch.setattr(proof_queue, "RUNNING_CHILD_MISSING_STALE_LOG_SECONDS", 0.0)

    rc = proof_queue.main(
        [
            "--db",
            str(db),
            "--logs-root",
            str(logs),
            "--repo-root",
            str(proof_queue.ROOT),
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

    assert rc == proof_queue.PROOF_QUEUE_STALE_EXIT_CODE
    out = capsys.readouterr().out
    assert "stale " in out
    assert "rc=?" in out
    rows = _rows(db)
    assert rows[0]["status"] == "stale"
    assert rows[0]["returncode"] is None
    assert popen_instances
    fake_proc = popen_instances[0]
    assert fake_proc.terminated
    assert not fake_proc.killed
    log_text = Path(rows[0]["log_path"]).read_text(encoding="utf-8")
    assert "proof_queue stale-running terminalization" in log_text
    assert "diagnosis=running-proof-child-missing" in log_text
    assert f"child_pid={child_pid}" in log_text
    assert "proof_queue finished status=stale exit_code=?" in log_text


def test_proof_queue_prune_stale_run_id_preserves_unselected_active_rows(
    monkeypatch: pytest.MonkeyPatch,
    tmp_path: Path,
    capsys: pytest.CaptureFixture[str],
) -> None:
    db = tmp_path / "proof_queue.sqlite3"
    stale_mtime = (
        time.time() - proof_queue.RUNNING_CHILD_MISSING_STALE_LOG_SECONDS - 5.0
    )
    conn = proof_queue._connect(db)
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
        proof_queue._insert_run(
            conn,
            run_id=run_id,
            logical_id=run_id,
            reason="prove targeted stale pruning",
            command=[sys.executable, "-c", "print('active')"],
            cwd=proof_queue.ROOT,
            resource_family="python-tests",
            contention_key="proof-queue-dx",
            scopes=["tools/proof_queue.py"],
            log_path=log_path,
            summary_json=summary_path,
        )
        proof_queue._update_run(
            conn,
            run_id,
            status="running",
            guard_pid=guard_pid,
            started_at=proof_queue._utc_now(),
        )
    monkeypatch.setattr(proof_queue, "_pid_alive", lambda pid: pid in {99_001, 99_002})

    assert (
        proof_queue.main(
            [
                "--db",
                str(db),
                "--logs-root",
                str(tmp_path / "runs"),
                "--repo-root",
                str(proof_queue.ROOT),
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
    statuses = {row["run_id"]: row["status"] for row in _rows(db)}
    assert statuses == {"target-run": "stale", "sibling-run": "running"}


def test_proof_queue_wasm_rows_ensure_rust_target_before_run(
    monkeypatch: pytest.MonkeyPatch, tmp_path: Path
) -> None:
    db = tmp_path / "proof_queue.sqlite3"
    logs = tmp_path / "runs"
    calls: list[tuple[str, Path | None]] = []

    def fake_ensure(
        target: str, warnings: list[str], *, root: Path | None = None
    ) -> bool:
        del warnings
        calls.append((target, root))
        return True

    monkeypatch.setattr(proof_queue.wasm_toolchain, "ensure_rustup_target", fake_ensure)

    rc = proof_queue.main(
        [
            "--db",
            str(db),
            "--logs-root",
            str(logs),
            "--repo-root",
            str(proof_queue.ROOT),
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
    assert calls == [
        (target, proof_queue.ROOT)
        for target in proof_queue.wasm_toolchain.rust_toolchain_contract(
            proof_queue.ROOT
        ).required_wasm_targets
    ]
    assert ("wasm32-wasip1", proof_queue.ROOT) in calls
    rows = _rows(db)
    assert rows[0]["status"] == "passed"
    assert "ran" in Path(rows[0]["log_path"]).read_text(encoding="utf-8")


def test_proof_queue_wasm_preflight_fails_before_command(
    monkeypatch: pytest.MonkeyPatch, tmp_path: Path
) -> None:
    db = tmp_path / "proof_queue.sqlite3"
    logs = tmp_path / "runs"

    def fake_ensure(
        target: str, warnings: list[str], *, root: Path | None = None
    ) -> bool:
        del root
        warnings.append(f"missing {target}")
        return False

    monkeypatch.setattr(proof_queue.wasm_toolchain, "ensure_rustup_target", fake_ensure)

    rc = proof_queue.main(
        [
            "--db",
            str(db),
            "--logs-root",
            str(logs),
            "--repo-root",
            str(proof_queue.ROOT),
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
    conn = proof_queue._connect(db)
    for run_id, marker in (("queued-a", "A"), ("queued-b", "B")):
        proof_queue._insert_run(
            conn,
            run_id=run_id,
            logical_id=run_id,
            reason=f"run {marker}",
            command=[sys.executable, "-c", f"print('{marker}')"],
            cwd=proof_queue.ROOT,
            resource_family="python",
            contention_key=f"python:{marker}",
            scopes=[],
            log_path=logs / f"{run_id}.log",
            summary_json=logs / f"{run_id}.memory_guard.json",
        )

    rc = proof_queue.main(
        [
            "--db",
            str(db),
            "--logs-root",
            str(logs),
            "--repo-root",
            str(proof_queue.ROOT),
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


def test_proof_queue_run_id_can_detach_existing_queued_row(
    monkeypatch: pytest.MonkeyPatch,
    tmp_path: Path,
    capsys: pytest.CaptureFixture[str],
) -> None:
    db = tmp_path / "proof_queue.sqlite3"
    logs = tmp_path / "runs"
    conn = proof_queue._connect(db)
    for run_id, marker in (("queued-a", "A"), ("queued-b", "B")):
        proof_queue._insert_run(
            conn,
            run_id=run_id,
            logical_id=run_id,
            reason=f"run {marker}",
            command=[sys.executable, "-c", f"print('{marker}')"],
            cwd=proof_queue.ROOT,
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

    monkeypatch.setattr(proof_queue, "_launch_detached_runner", fake_launch)

    rc = proof_queue.main(
        [
            "--db",
            str(db),
            "--logs-root",
            str(logs),
            "--repo-root",
            str(proof_queue.ROOT),
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
    assert rows["queued-b"]["status"] == "queued"
    assert not (logs / "queued-b.log").exists()
    assert "detached queued-b runner_pid=12345" in stdout
    assert f"runner_log: {logs / 'queued-b.runner.log'}" in stdout


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

    monkeypatch.setattr(proof_queue, "_launch_detached_runner", fake_launch)

    rc = proof_queue.main(
        [
            "--db",
            str(db),
            "--logs-root",
            str(logs),
            "--repo-root",
            str(proof_queue.ROOT),
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
    assert rows[0]["status"] == "queued"
    assert launched == {"run_id": rows[0]["run_id"], "timeout": 42.0}
    assert [note["body"] for note in _notes(db)][-1:] == ["detached queue launch smoke"]


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

    monkeypatch.setattr(proof_queue, "_launch_detached_runner", fail_launch)

    rc = proof_queue.main(
        [
            "--db",
            str(db),
            "--logs-root",
            str(logs),
            "--repo-root",
            str(proof_queue.ROOT),
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
    assert [note["body"] for note in _notes(db)][-1:] == [
        "queue-only R6 parking smoke"
    ]


def test_proof_queue_named_lane_rejects_queue_only_with_detach(
    tmp_path: Path,
) -> None:
    db = tmp_path / "proof_queue.sqlite3"
    logs = tmp_path / "runs"

    with pytest.raises(SystemExit) as exc:
        proof_queue.main(
            [
                "--db",
                str(db),
                "--logs-root",
                str(logs),
                "--repo-root",
                str(proof_queue.ROOT),
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
        proof_queue.main(
            [
                "--db",
                str(db),
                "--logs-root",
                str(logs),
                "--repo-root",
                str(proof_queue.ROOT),
                "r6-target-version-parity",
                "--queue-only",
            ]
        )
        == 0
    )
    row = _rows(db)[0]
    Path(row["log_path"]).unlink()

    assert proof_queue._active_log_status(row) == [
        f"  log={Path(row['log_path'])} (queued; proof command not launched yet)"
    ]
    capsys.readouterr()
    assert (
        proof_queue.main(
            [
                "--db",
                str(db),
                "--logs-root",
                str(logs),
                "--repo-root",
                str(proof_queue.ROOT),
                "audit",
                "--json",
                "--no-notebook-check",
            ]
        )
        == 0
    )
    payload = json.loads(capsys.readouterr().out)
    assert all(
        issue["signal_id"] != "audit-active-log-missing"
        for issue in payload["issues"]
    )


def test_proof_queue_windows_launchers_hide_console(
    monkeypatch: pytest.MonkeyPatch, tmp_path: Path
) -> None:
    captured: list[dict[str, object]] = []

    class FakePopen:
        pid = 12345

        def __init__(self, _command: list[str], **kwargs: object) -> None:
            captured.append(kwargs)

    monkeypatch.setattr(proof_queue.os, "name", "nt")
    monkeypatch.setattr(
        proof_queue.subprocess,
        "CREATE_NEW_PROCESS_GROUP",
        0x00000200,
        raising=False,
    )
    monkeypatch.setattr(
        proof_queue.subprocess,
        "CREATE_NO_WINDOW",
        0x08000000,
        raising=False,
    )
    monkeypatch.setattr(proof_queue.subprocess, "Popen", FakePopen)

    args = SimpleNamespace(
        db=tmp_path / "proof_queue.sqlite3",
        logs_root=tmp_path / "runs",
        notebooks_root=None,
        repo_root=tmp_path,
    )

    proof_queue._launch_detached_runner(args, run_id="hidden-runner", timeout=1.0)

    assert captured[0]["creationflags"] == 0x08000200
    assert proof_queue._queued_command_process_kwargs() == {"creationflags": 0x08000200}


def test_proof_queue_rejects_uv_run_without_active_project_python(
    tmp_path: Path,
) -> None:
    db = tmp_path / "proof_queue.sqlite3"
    logs = tmp_path / "runs"

    rc = proof_queue.main(
        [
            "--db",
            str(db),
            "--logs-root",
            str(logs),
            "--repo-root",
            str(proof_queue.ROOT),
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

    rc = proof_queue.main(
        [
            "--db",
            str(db),
            "--logs-root",
            str(logs),
            "--repo-root",
            str(proof_queue.ROOT),
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
        proof_queue.main(
            [
                "--db",
                str(db),
                "--logs-root",
                str(logs),
                "--repo-root",
                str(proof_queue.ROOT),
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

    monkeypatch.setattr(proof_queue, "_launch_detached_runner", fake_launch)

    rc = proof_queue.main(
        [
            "--db",
            str(db),
            "--logs-root",
            str(logs),
            "--repo-root",
            str(proof_queue.ROOT),
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
    assert rows[0]["status"] == "queued"
    assert rows[0]["resource_family"] == "rust"
    assert rows[0]["contention_key"] == "cargo:molt-runtime"
    assert launched == {"run_id": rows[0]["run_id"], "timeout": 42.0}
    command = json.loads(rows[0]["command_json"])
    assert command[:8] == [
        "uv",
        "run",
        "--active",
        "--project",
        ".",
        "--python",
        "3.12",
        "python",
    ]
    assert command[8:14] == [
        "tools/guarded_exec.py",
        "--prefix",
        "MOLT_TEST_SUITE",
        "--",
        "cargo",
        "test",
    ]
    assert command[14:16] == ["-p", "molt-runtime"]
    assert command[-1] == "--lib"
    assert [note["body"] for note in _notes(db)] == ["canonical cargo proof lane smoke"]


def test_proof_queue_cargo_rejects_pre_delimiter_residue(tmp_path: Path) -> None:
    db = tmp_path / "proof_queue.sqlite3"
    logs = tmp_path / "runs"

    with pytest.raises(SystemExit, match="stray positional argument"):
        proof_queue.main(
            [
                "--db",
                str(db),
                "--logs-root",
                str(logs),
                "--repo-root",
                str(proof_queue.ROOT),
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
        proof_queue.main(
            [
                "--db",
                str(db),
                "--logs-root",
                str(logs),
                "--repo-root",
                str(proof_queue.ROOT),
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

    monkeypatch.setattr(proof_queue, "_launch_detached_runner", fake_launch)

    rc = proof_queue.main(
        [
            "--db",
            str(db),
            "--logs-root",
            str(logs),
            "--repo-root",
            str(proof_queue.ROOT),
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

    monkeypatch.setattr(proof_queue, "_launch_detached_runner", fake_launch)

    rc = proof_queue.main(
        [
            "--db",
            str(db),
            "--logs-root",
            str(logs),
            "--repo-root",
            str(proof_queue.ROOT),
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
    assert rows[0]["status"] == "queued"
    assert launched == {"run_id": rows[0]["run_id"], "timeout": 42.0}
    command = json.loads(rows[0]["command_json"])
    assert "pyinit_module_to_bits_reports_static_link_py_mod_exec_pending_error" in command
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
        proof_queue.main(
            [
                "--db",
                str(db),
                "--logs-root",
                str(logs),
                "--repo-root",
                str(proof_queue.ROOT),
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
        proof_queue.main(
            [
                "--db",
                str(db),
                "--logs-root",
                str(logs),
                "--repo-root",
                str(proof_queue.ROOT),
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
        proof_queue.main(
            [
                "--db",
                str(db),
                "--logs-root",
                str(logs),
                "--notebooks-root",
                str(notebooks),
                "--repo-root",
                str(proof_queue.ROOT),
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
        proof_queue.main(
            [
                "--db",
                str(db),
                "--logs-root",
                str(logs),
                "--notebooks-root",
                str(notebooks),
                "--repo-root",
                str(proof_queue.ROOT),
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
        proof_queue.main(
            [
                "--db",
                str(db),
                "--logs-root",
                str(logs),
                "--notebooks-root",
                str(notebooks),
                "--repo-root",
                str(proof_queue.ROOT),
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
        proof_queue.main(
            [
                "--db",
                str(db),
                "--logs-root",
                str(logs),
                "--notebooks-root",
                str(notebooks),
                "--repo-root",
                str(proof_queue.ROOT),
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
    conn = proof_queue._connect(db)
    for run_id, status in (("failed-parent", "failed"), ("blocked-child", "queued")):
        proof_queue._insert_run(
            conn,
            run_id=run_id,
            logical_id=run_id,
            reason="prove blocked dependency evidence",
            command=[sys.executable, "-c", "print('blocked')"],
            cwd=proof_queue.ROOT,
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
        proof_queue._update_run(conn, run_id, status=status)
    proof_queue._insert_note(
        conn,
        run_id="blocked-child",
        body="test: blocked dependency must leave evidence",
        kind="submission",
        author="codex",
    )
    proof_queue._insert_edge(
        conn,
        parent_run_id="failed-parent",
        child_run_id="blocked-child",
        kind="depends_on",
        note="child waits on failed parent",
    )

    assert (
        proof_queue.main(
            [
                "--db",
                str(db),
                "--logs-root",
                str(logs),
                "--notebooks-root",
                str(notebooks),
                "--repo-root",
                str(proof_queue.ROOT),
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
        proof_queue.main(
            [
                "--db",
                str(db),
                "--logs-root",
                str(logs),
                "--repo-root",
                str(proof_queue.ROOT),
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
        proof_queue.main(
            [
                "--db",
                str(db),
                "--logs-root",
                str(logs),
                "--repo-root",
                str(proof_queue.ROOT),
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
        proof_queue.main(
            [
                "--db",
                str(db),
                "--logs-root",
                str(logs),
                "--notebooks-root",
                str(notebooks),
                "--repo-root",
                str(proof_queue.ROOT),
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
        repo_root=proof_queue.ROOT,
    )

    rc, run_id = proof_queue._queue_one(
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
    conn = proof_queue._connect(db)
    for run_id, status in (("failed-parent", "failed"), ("rerun-child", "queued")):
        proof_queue._insert_run(
            conn,
            run_id=run_id,
            logical_id=run_id,
            reason="prove lineage edges never gate",
            command=[sys.executable, "-c", "print('rerun')"],
            cwd=proof_queue.ROOT,
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
        proof_queue._update_run(conn, run_id, status=status)
    # A rerun's parent is failed or stale by definition: lineage kinds
    # preserve provenance and must never gate scheduling (PROOF_QUEUE.md:
    # "depends_on is the scheduling edge; the others preserve lineage").
    for kind in ("reruns", "supersedes", "compares", "derives_from"):
        proof_queue._insert_edge(
            conn,
            parent_run_id="failed-parent",
            child_run_id="rerun-child",
            kind=kind,
            note=f"lineage edge {kind}",
        )

    state, blockers = proof_queue._dependency_state(conn, "rerun-child")

    assert state == "ready"
    assert blockers == []

    proof_queue._insert_edge(
        conn,
        parent_run_id="failed-parent",
        child_run_id="rerun-child",
        kind="depends_on",
        note="scheduling edge still gates",
    )
    state, blockers = proof_queue._dependency_state(conn, "rerun-child")
    assert state == "blocked"
    assert [row["kind"] for row in blockers] == ["depends_on"]


def test_proof_queue_appends_notes_and_exports_evidence(
    tmp_path: Path, capsys: pytest.CaptureFixture[str]
) -> None:
    db = tmp_path / "proof_queue.sqlite3"
    notebooks = tmp_path / "notebooks"
    conn = proof_queue._connect(db)
    proof_queue._insert_run(
        conn,
        run_id="noted-run",
        logical_id="noted",
        reason="prove append-only notes",
        command=[sys.executable, "-c", "print('noted')"],
        cwd=proof_queue.ROOT,
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
        proof_queue.main(
            [
                "--db",
                str(db),
                "--logs-root",
                str(tmp_path / "runs"),
                "--notebooks-root",
                str(notebooks),
                "--repo-root",
                str(proof_queue.ROOT),
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
        proof_queue.main(
            [
                "--db",
                str(db),
                "--logs-root",
                str(tmp_path / "runs"),
                "--repo-root",
                str(proof_queue.ROOT),
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
    conn = proof_queue._connect(db)
    proof_queue._insert_run(
        conn,
        run_id="noted-warning-run",
        logical_id="noted-warning",
        reason="prove note survives notebook projection failure",
        command=[sys.executable, "-c", "print('noted')"],
        cwd=proof_queue.ROOT,
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

    monkeypatch.setattr(proof_queue, "_write_marimo_notebook", fail_notebook)

    assert (
        proof_queue.main(
            [
                "--db",
                str(db),
                "--logs-root",
                str(tmp_path / "runs"),
                "--repo-root",
                str(proof_queue.ROOT),
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
    conn = proof_queue._connect(db)
    proof_queue._insert_run(
        conn,
        run_id="failed-run",
        logical_id="pact-witness-acceptance",
        reason="prove runtime export obligation diagnosis",
        command=[sys.executable, "-c", "print('fail')"],
        cwd=proof_queue.ROOT,
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
    proof_queue._update_run(conn, "failed-run", status="failed", returncode=1)

    assert (
        proof_queue.main(
            [
                "--db",
                str(db),
                "--logs-root",
                str(tmp_path / "runs"),
                "--repo-root",
                str(proof_queue.ROOT),
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
    conn = proof_queue._connect(db)
    proof_queue._insert_run(
        conn,
        run_id="failed-run",
        logical_id="pact-witness-acceptance",
        reason="prove export authority unknown-name diagnosis",
        command=[sys.executable, "-c", "print('fail')"],
        cwd=proof_queue.ROOT,
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
    proof_queue._update_run(conn, "failed-run", status="failed", returncode=1)

    assert (
        proof_queue.main(
            [
                "--db",
                str(db),
                "--logs-root",
                str(tmp_path / "runs"),
                "--repo-root",
                str(proof_queue.ROOT),
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
    conn = proof_queue._connect(db)
    proof_queue._insert_run(
        conn,
        run_id="failed-run",
        logical_id="pact-witness-acceptance",
        reason="prove deterministic diagnosis",
        command=[sys.executable, "-c", "print('fail')"],
        cwd=proof_queue.ROOT,
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
    proof_queue._update_run(conn, "failed-run", status="failed", returncode=1)

    assert (
        proof_queue.main(
            [
                "--db",
                str(db),
                "--logs-root",
                str(tmp_path / "runs"),
                "--repo-root",
                str(proof_queue.ROOT),
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
        proof_queue.main(
            [
                "--db",
                str(db),
                "--logs-root",
                str(tmp_path / "runs"),
                "--notebooks-root",
                str(notebooks),
                "--repo-root",
                str(proof_queue.ROOT),
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


def test_proof_queue_diagnoses_rust_compile_error_and_guard_orphan_cleanup(
    tmp_path: Path, capsys: pytest.CaptureFixture[str]
) -> None:
    db = tmp_path / "proof_queue.sqlite3"
    log_path = tmp_path / "rust-failed.log"
    conn = proof_queue._connect(db)
    proof_queue._insert_run(
        conn,
        run_id="rust-failed-run",
        logical_id="rust-failed",
        reason="prove Rust compiler diagnostics",
        command=["cargo", "test", "-p", "molt-runtime"],
        cwd=proof_queue.ROOT,
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
    proof_queue._insert_note(
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
            ]
        ),
        encoding="utf-8",
    )
    proof_queue._update_run(conn, "rust-failed-run", status="failed", returncode=101)

    assert (
        proof_queue.main(
            [
                "--db",
                str(db),
                "--logs-root",
                str(tmp_path / "runs"),
                "--repo-root",
                str(proof_queue.ROOT),
                "evidence",
                "--run-id",
                "rust-failed-run",
            ]
        )
        == 0
    )
    evidence = json.loads(capsys.readouterr().out)
    signals = [item["signal_id"] for item in evidence[0]["diagnostics"]]
    assert signals[:2] == ["rust-compiler-error", "memory-guard-orphan-cleanup"]


def test_proof_queue_diagnoses_memory_guard_timeout_before_orphan_cleanup(
    tmp_path: Path, capsys: pytest.CaptureFixture[str]
) -> None:
    db = tmp_path / "proof_queue.sqlite3"
    log_path = tmp_path / "timeout.log"
    summary_path = tmp_path / "timeout.memory_guard.json"
    nodeid = (
        "tests/test_native_import_bootstrap_regressions.py::"
        "test_native_package_init_try_guard_uses_nameerror_lookup"
    )
    conn = proof_queue._connect(db)
    proof_queue._insert_run(
        conn,
        run_id="timeout-run",
        logical_id="native-import-typeerror-current-recheck",
        reason="prove timeout diagnosis outranks orphan cleanup",
        command=[
            sys.executable,
            "-m",
            "pytest",
            "tests/test_native_import_bootstrap_regressions.py",
        ],
        cwd=proof_queue.ROOT,
        resource_family="python-tests",
        contention_key="native-import-regression",
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
    proof_queue._insert_note(
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
    proof_queue._update_run(conn, "timeout-run", status="failed", returncode=124)

    assert (
        proof_queue.main(
            [
                "--db",
                str(db),
                "--logs-root",
                str(tmp_path / "runs"),
                "--repo-root",
                str(proof_queue.ROOT),
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
    assert "test_native_package_init_try_guard_uses_nameerror_lookup" in diagnostics[0]["summary"]
    assert "pytest_phase=call" in diagnostics[0]["evidence"]
    assert f"Inspect {nodeid} once" in diagnostics[0]["next_action"]


def test_proof_queue_diagnoses_pytest_assertion_failure(
    tmp_path: Path, capsys: pytest.CaptureFixture[str]
) -> None:
    db = tmp_path / "proof_queue.sqlite3"
    log_path = tmp_path / "pytest-failed.log"
    conn = proof_queue._connect(db)
    proof_queue._insert_run(
        conn,
        run_id="pytest-failed-run",
        logical_id="pytest-failed",
        reason="prove pytest diagnostics",
        command=[sys.executable, "-m", "pytest", "tests/test_wasm_link_validation.py"],
        cwd=proof_queue.ROOT,
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
    proof_queue._insert_note(
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
    proof_queue._update_run(conn, "pytest-failed-run", status="failed", returncode=1)

    assert (
        proof_queue.main(
            [
                "--db",
                str(db),
                "--logs-root",
                str(tmp_path / "runs"),
                "--repo-root",
                str(proof_queue.ROOT),
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


def test_proof_queue_diagnoses_cold_single_cargo_proof_policy_refusal(
    tmp_path: Path, capsys: pytest.CaptureFixture[str]
) -> None:
    db = tmp_path / "proof_queue.sqlite3"
    log_path = tmp_path / "cold-single-cargo.log"
    conn = proof_queue._connect(db)
    proof_queue._insert_run(
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
        cwd=proof_queue.ROOT,
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
    proof_queue._insert_note(
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
    proof_queue._update_run(
        conn,
        "cold-single-cargo-run",
        status="failed",
        returncode=2,
    )

    assert (
        proof_queue.main(
            [
                "--db",
                str(db),
                "--logs-root",
                str(tmp_path / "runs"),
                "--repo-root",
                str(proof_queue.ROOT),
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
    conn = proof_queue._connect(db)
    proof_queue._insert_run(
        conn,
        run_id="molt-runtime-invalid-header-run",
        logical_id="native-module-dunder-cleanup-trace",
        reason="prove Molt runtime fatal diagnostics",
        command=[str(tmp_path / "compiled-native-binary")],
        cwd=proof_queue.ROOT,
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
    proof_queue._insert_note(
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
    proof_queue._update_run(
        conn,
        "molt-runtime-invalid-header-run",
        status="failed",
        returncode=4294967295,
    )

    assert (
        proof_queue.main(
            [
                "--db",
                str(db),
                "--logs-root",
                str(tmp_path / "runs"),
                "--repo-root",
                str(proof_queue.ROOT),
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


def test_proof_queue_diagnoses_runtime_wasm_rust_target_missing(
    tmp_path: Path, capsys: pytest.CaptureFixture[str]
) -> None:
    db = tmp_path / "proof_queue.sqlite3"
    log_path = tmp_path / "runtime-wasm-rust-target.log"
    conn = proof_queue._connect(db)
    proof_queue._insert_run(
        conn,
        run_id="runtime-wasm-rust-target-run",
        logical_id="pact-witness-acceptance",
        reason="prove missing wasm Rust target diagnosis",
        command=[sys.executable, "tools/pact_witness_acceptance.py"],
        cwd=proof_queue.ROOT,
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
    log_path.write_text(
        "Runtime wasm build requires Rust target wasm32-wasip1, but the active "
        "Rust toolchain does not provide it. Run: rustup target add "
        "wasm32-wasip1 --toolchain 1.96.1\n"
        "Runtime wasm build failed\n"
        "subprocess.CalledProcessError: Command '['python', '-m', 'molt', "
        "'build']' returned non-zero exit status 2.\n",
        encoding="utf-8",
    )
    proof_queue._update_run(
        conn, "runtime-wasm-rust-target-run", status="failed", returncode=1
    )

    assert (
        proof_queue.main(
            [
                "--db",
                str(db),
                "--logs-root",
                str(tmp_path / "runs"),
                "--repo-root",
                str(proof_queue.ROOT),
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
    assert "python-exception" not in {item["signal_id"] for item in diagnostics}
    assert "unclassified-failed-proof" not in {
        item["signal_id"] for item in diagnostics
    }


def test_proof_queue_diagnoses_source_lease_contamination(
    tmp_path: Path, capsys: pytest.CaptureFixture[str]
) -> None:
    db = tmp_path / "proof_queue.sqlite3"
    log_path = tmp_path / "source-lease.log"
    conn = proof_queue._connect(db)
    proof_queue._insert_run(
        conn,
        run_id="source-lease-run",
        logical_id="source-lease",
        reason="prove source lease contamination diagnosis",
        command=[sys.executable, "tests/molt_diff.py", "case.py"],
        cwd=proof_queue.ROOT,
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
    proof_queue._update_run(conn, "source-lease-run", status="failed", returncode=1)

    assert (
        proof_queue.main(
            [
                "--db",
                str(db),
                "--logs-root",
                str(tmp_path / "runs"),
                "--repo-root",
                str(proof_queue.ROOT),
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
    conn = proof_queue._connect(db)
    proof_queue._insert_run(
        conn,
        run_id="partial-module-publication-run",
        logical_id="partial-module-publication",
        reason="prove partial module publication diagnosis",
        command=[sys.executable, "tests/molt_diff.py", "case.py"],
        cwd=proof_queue.ROOT,
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
        "ImportError: cannot import partially initialized module "
        "'importlib.machinery' before its publication "
        "(circular import during module allocation)\n",
        encoding="utf-8",
    )
    proof_queue._update_run(
        conn, "partial-module-publication-run", status="failed", returncode=1
    )

    assert (
        proof_queue.main(
            [
                "--db",
                str(db),
                "--logs-root",
                str(tmp_path / "runs"),
                "--repo-root",
                str(proof_queue.ROOT),
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
    assert "runtime/molt-runtime/src/builtins/module_table.rs" in diagnostics[0][
        "scopes"
    ]
    assert "unclassified-failed-proof" not in {
        item["signal_id"] for item in diagnostics
    }


def test_proof_queue_diagnoses_pytest_import_error(
    tmp_path: Path, capsys: pytest.CaptureFixture[str]
) -> None:
    db = tmp_path / "proof_queue.sqlite3"
    log_path = tmp_path / "pytest-import-error.log"
    conn = proof_queue._connect(db)
    proof_queue._insert_run(
        conn,
        run_id="pytest-import-error-run",
        logical_id="pytest-import-error",
        reason="prove pytest import diagnostics",
        command=[sys.executable, "-m", "pytest", "tests/test_molt_dev.py"],
        cwd=proof_queue.ROOT,
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
    proof_queue._insert_note(
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
    proof_queue._update_run(
        conn, "pytest-import-error-run", status="failed", returncode=1
    )

    assert (
        proof_queue.main(
            [
                "--db",
                str(db),
                "--logs-root",
                str(tmp_path / "runs"),
                "--repo-root",
                str(proof_queue.ROOT),
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
    conn = proof_queue._connect(db)
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
        proof_queue._insert_run(
            conn,
            run_id=run_id,
            logical_id="pact-witness-acceptance",
            reason="prove external native diagnostics",
            command=[sys.executable, "-c", "raise SystemExit(2)"],
            cwd=proof_queue.ROOT,
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
        proof_queue._insert_note(
            conn,
            run_id=run_id,
            body="test: classify recurring Pact build refusal",
            kind="submission",
            author="codex",
        )
        log_path.write_text(log_text + "\n", encoding="utf-8")
        proof_queue._update_run(conn, run_id, status="failed", returncode=2)

    assert (
        proof_queue.main(
            [
                "--db",
                str(db),
                "--logs-root",
                str(tmp_path / "runs"),
                "--repo-root",
                str(proof_queue.ROOT),
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


def test_proof_queue_diagnoses_external_native_abi_link_surface_gap(
    tmp_path: Path, capsys: pytest.CaptureFixture[str]
) -> None:
    db = tmp_path / "proof_queue.sqlite3"
    log_path = tmp_path / "abi-link-surface.log"
    conn = proof_queue._connect(db)
    proof_queue._insert_run(
        conn,
        run_id="abi-link-surface",
        logical_id="pact-witness-acceptance",
        reason="prove generated ABI link surface diagnostics",
        command=[sys.executable, "-c", "raise SystemExit(2)"],
        cwd=proof_queue.ROOT,
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
    proof_queue._insert_note(
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
    proof_queue._update_run(conn, "abi-link-surface", status="failed", returncode=2)

    assert (
        proof_queue.main(
            [
                "--db",
                str(db),
                "--logs-root",
                str(tmp_path / "runs"),
                "--repo-root",
                str(proof_queue.ROOT),
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
    conn = proof_queue._connect(db)
    proof_queue._insert_run(
        conn,
        run_id="classified-run",
        logical_id="pact-witness-acceptance",
        reason="prove classified product failure is not queue debt",
        command=[sys.executable, "-c", "raise SystemExit(1)"],
        cwd=proof_queue.ROOT,
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
    proof_queue._insert_note(
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
    proof_queue._update_run(conn, "classified-run", status="failed", returncode=1)

    assert (
        proof_queue.main(
            [
                "--db",
                str(db),
                "--logs-root",
                str(tmp_path / "runs"),
                "--repo-root",
                str(proof_queue.ROOT),
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
    conn = proof_queue._connect(db)
    proof_queue._insert_run(
        conn,
        run_id="weak-metadata-run",
        logical_id="bad-shell-quote",
        reason='"Prove',
        command=[sys.executable, "-c", "print('passed with weak metadata')"],
        cwd=proof_queue.ROOT,
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
    proof_queue._update_run(
        conn,
        "weak-metadata-run",
        status="passed",
        returncode=0,
    )

    assert (
        proof_queue.main(
            [
                "--db",
                str(db),
                "--logs-root",
                str(tmp_path / "runs"),
                "--repo-root",
                str(proof_queue.ROOT),
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
    conn = proof_queue._connect(db)
    product_log = tmp_path / "frontier.log"
    warning_log = tmp_path / "guard-warning.log"
    proof_queue._insert_run(
        conn,
        run_id="frontier-run",
        logical_id="pact-witness-acceptance",
        reason="prove audit product frontier",
        command=[sys.executable, "-c", "raise SystemExit(2)"],
        cwd=proof_queue.ROOT,
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
    proof_queue._insert_note(
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
    proof_queue._update_run(conn, "frontier-run", status="failed", returncode=2)

    proof_queue._insert_run(
        conn,
        run_id="guard-warning-run",
        logical_id="guard-warning",
        reason="prove audit warning noise does not hide frontier",
        command=[sys.executable, "-c", "print('ok')"],
        cwd=proof_queue.ROOT,
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
    proof_queue._insert_note(
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
    proof_queue._update_run(conn, "guard-warning-run", status="passed", returncode=0)

    assert (
        proof_queue.main(
            [
                "--db",
                str(db),
                "--logs-root",
                str(tmp_path / "runs"),
                "--repo-root",
                str(proof_queue.ROOT),
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
    conn = proof_queue._connect(db)

    error_log = tmp_path / "error.log"
    proof_queue._insert_run(
        conn,
        run_id="error-run",
        logical_id="error-run",
        reason="prove audit errors-only still surfaces errors",
        command=[sys.executable, "-c", "raise SystemExit(1)"],
        cwd=proof_queue.ROOT,
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
    proof_queue._insert_note(
        conn,
        run_id="error-run",
        body="test: unclassified errors stay visible under errors-only",
        kind="submission",
        author="codex",
    )
    error_log.write_text("mystery failure without a diagnostic\n", encoding="utf-8")
    proof_queue._update_run(conn, "error-run", status="failed", returncode=1)

    warning_log = tmp_path / "warning.log"
    proof_queue._insert_run(
        conn,
        run_id="warning-run",
        logical_id="warning-run",
        reason="prove audit errors-only filters warning rows",
        command=[sys.executable, "-c", "print('ok')"],
        cwd=proof_queue.ROOT,
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
    proof_queue._insert_note(
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
    proof_queue._update_run(conn, "warning-run", status="passed", returncode=0)

    assert (
        proof_queue.main(
            [
                "--db",
                str(db),
                "--logs-root",
                str(tmp_path / "runs"),
                "--repo-root",
                str(proof_queue.ROOT),
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
    conn = proof_queue._connect(db)
    for run_id, status in (
        ("stale-failure", "failed"),
        ("rerun-child", "passed"),
        ("current-failure", "failed"),
    ):
        log_path = tmp_path / f"{run_id}.log"
        proof_queue._insert_run(
            conn,
            run_id=run_id,
            logical_id="pact-witness-acceptance",
            reason="prove superseded frontier filtering",
            command=[sys.executable, "-c", "raise SystemExit(1)"],
            cwd=proof_queue.ROOT,
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
        proof_queue._insert_note(
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
        proof_queue._update_run(
            conn,
            run_id,
            status=status,
            returncode=0 if status == "passed" else 1,
        )
    proof_queue._insert_edge(
        conn,
        parent_run_id="stale-failure",
        child_run_id="rerun-child",
        kind="reruns",
        note="rerun retired stale frontier",
        author="codex",
    )

    assert (
        proof_queue.main(
            [
                "--db",
                str(db),
                "--logs-root",
                str(tmp_path / "runs"),
                "--repo-root",
                str(proof_queue.ROOT),
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
    conn = proof_queue._connect(db)
    for run_id, status in (
        ("stale-timeout", "failed"),
        ("rerun-child", "passed"),
    ):
        log_path = tmp_path / f"{run_id}.log"
        proof_queue._insert_run(
            conn,
            run_id=run_id,
            logical_id="memory-guard-dx",
            reason="prove superseded queue debt filtering",
            command=[sys.executable, "-c", "print('proof')"],
            cwd=proof_queue.ROOT,
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
        proof_queue._insert_note(
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
        proof_queue._update_run(
            conn,
            run_id,
            status=status,
            returncode=0 if status == "passed" else 124,
        )
    proof_queue._insert_edge(
        conn,
        parent_run_id="stale-timeout",
        child_run_id="rerun-child",
        kind="reruns",
        note="rerun retired stale timeout",
        author="codex",
    )

    assert (
        proof_queue.main(
            [
                "--db",
                str(db),
                "--logs-root",
                str(tmp_path / "runs"),
                "--repo-root",
                str(proof_queue.ROOT),
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
        proof_queue.main(
            [
                "--db",
                str(db),
                "--logs-root",
                str(tmp_path / "runs"),
                "--repo-root",
                str(proof_queue.ROOT),
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
        conn = proof_queue._connect(db)
        parent_run_id = f"stale-timeout-{kind}-{child_status}"
        child_run_id = f"child-{kind}-{child_status}"
        for run_id, status in (
            (parent_run_id, "failed"),
            (child_run_id, child_status),
        ):
            log_path = tmp_path / f"{run_id}.log"
            proof_queue._insert_run(
                conn,
                run_id=run_id,
                logical_id="memory-guard-dx",
                reason="prove audit retirement stays narrow",
                command=[sys.executable, "-c", "print('proof')"],
                cwd=proof_queue.ROOT,
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
            proof_queue._insert_note(
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
            proof_queue._update_run(
                conn,
                run_id,
                status=status,
                returncode=124 if status == "failed" else 0,
            )
        proof_queue._insert_edge(
            conn,
            parent_run_id=parent_run_id,
            child_run_id=child_run_id,
            kind=kind,
            note="edge must not over-retire parent failure",
            author="codex",
        )

        assert (
            proof_queue.main(
                [
                    "--db",
                    str(db),
                    "--logs-root",
                    str(tmp_path / "runs"),
                    "--repo-root",
                    str(proof_queue.ROOT),
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
    conn = proof_queue._connect(db)
    proof_queue._insert_run(
        conn,
        run_id="mystery-run",
        logical_id="mystery",
        reason="prove queue audit catches unclassified rows",
        command=[sys.executable, "-c", "raise SystemExit(1)"],
        cwd=proof_queue.ROOT,
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
    proof_queue._insert_note(
        conn,
        run_id="mystery-run",
        body="test: unclassified failure must be queue debt",
        kind="submission",
        author="codex",
    )
    log_path.write_text("mystery failure with no known diagnostic\n", encoding="utf-8")
    proof_queue._update_run(conn, "mystery-run", status="failed", returncode=1)

    assert (
        proof_queue.main(
            [
                "--db",
                str(db),
                "--logs-root",
                str(tmp_path / "runs"),
                "--repo-root",
                str(proof_queue.ROOT),
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
    conn = proof_queue._connect(db)
    for index in range(3):
        run_id = f"mystery-run-{index}"
        log_path = tmp_path / f"{run_id}.log"
        proof_queue._insert_run(
            conn,
            run_id=run_id,
            logical_id="mystery",
            reason="prove capped audit output",
            command=[sys.executable, "-c", "raise SystemExit(1)"],
            cwd=proof_queue.ROOT,
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
        proof_queue._insert_note(
            conn,
            run_id=run_id,
            body="test: unclassified failure must remain visible",
            kind="submission",
            author="codex",
        )
        log_path.write_text(
            "mystery failure with no known diagnostic\n", encoding="utf-8"
        )
        proof_queue._update_run(conn, run_id, status="failed", returncode=1)

    assert (
        proof_queue.main(
            [
                "--db",
                str(db),
                "--logs-root",
                str(tmp_path / "runs"),
                "--repo-root",
                str(proof_queue.ROOT),
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
    conn = proof_queue._connect(db)
    for run_id in ("parent-run", "child-run"):
        proof_queue._insert_run(
            conn,
            run_id=run_id,
            logical_id=run_id,
            reason="prove DAG link",
            command=[sys.executable, "-c", "print('dag')"],
            cwd=proof_queue.ROOT,
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
        proof_queue.main(
            [
                "--db",
                str(db),
                "--logs-root",
                str(tmp_path / "runs"),
                "--notebooks-root",
                str(notebooks),
                "--repo-root",
                str(proof_queue.ROOT),
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
        proof_queue.main(
            [
                "--db",
                str(db),
                "--logs-root",
                str(tmp_path / "runs"),
                "--repo-root",
                str(proof_queue.ROOT),
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
    conn = proof_queue._connect(db)
    for run_id in ("parent-warning-run", "child-warning-run"):
        proof_queue._insert_run(
            conn,
            run_id=run_id,
            logical_id=run_id,
            reason="prove DAG link survives notebook projection failure",
            command=[sys.executable, "-c", "print('dag')"],
            cwd=proof_queue.ROOT,
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

    monkeypatch.setattr(proof_queue, "_write_marimo_notebook", fail_notebook)

    assert (
        proof_queue.main(
            [
                "--db",
                str(db),
                "--logs-root",
                str(tmp_path / "runs"),
                "--repo-root",
                str(proof_queue.ROOT),
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
    conn = proof_queue._connect(db)
    proof_queue._insert_run(
        conn,
        run_id="kind-run",
        logical_id="kind",
        reason="prove note kind vocabulary",
        command=[sys.executable, "-c", "print('kind')"],
        cwd=proof_queue.ROOT,
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
        proof_queue._insert_note(
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
                proof_queue._utc_now(),
                "codex",
                "blocker",
                "raw sqlite path should fail closed",
            ),
        )


def test_proof_queue_notes_are_database_append_only(tmp_path: Path) -> None:
    db = tmp_path / "proof_queue.sqlite3"
    conn = proof_queue._connect(db)
    proof_queue._insert_run(
        conn,
        run_id="append-only-run",
        logical_id="append-only",
        reason="prove immutable notes table",
        command=[sys.executable, "-c", "print('append-only')"],
        cwd=proof_queue.ROOT,
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
    proof_queue._insert_note(
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
    conn = proof_queue._connect(db)
    for run_id in ("a-run", "b-run"):
        proof_queue._insert_run(
            conn,
            run_id=run_id,
            logical_id=run_id,
            reason="prove DAG guard",
            command=[sys.executable, "-c", "print('dag')"],
            cwd=proof_queue.ROOT,
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
    proof_queue._insert_edge(
        conn,
        parent_run_id="a-run",
        child_run_id="b-run",
        kind="depends_on",
        note="b waits on a",
    )

    with pytest.raises(SystemExit, match="would create a cycle"):
        proof_queue._insert_edge(
            conn,
            parent_run_id="b-run",
            child_run_id="a-run",
            kind="depends_on",
        )

    with pytest.raises(SystemExit, match="unknown proof edge kind"):
        proof_queue._insert_edge(
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
        proof_queue.main(
            [
                "--db",
                str(db),
                "--logs-root",
                str(logs),
                "--repo-root",
                str(proof_queue.ROOT),
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
        proof_queue.main(
            [
                "--db",
                str(db),
                "--logs-root",
                str(logs),
                "--repo-root",
                str(proof_queue.ROOT),
                "submit",
                str(dsl),
            ]
        )
    assert _rows(db) == []


def test_proof_queue_pact_witness_acceptance_is_queue_native() -> None:
    spec = proof_queue._pact_witness_acceptance_spec()

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
    assert command[7:9] == ["python", "tools/pact_witness_acceptance.py"]
    assert "tmp/pact_witness_acceptance_queue" in command
    assert "tools/pact_witness_acceptance.py" in spec["scopes"]
    assert "collab/pact/pact_witness_kernel/check_parity.py" in spec["scopes"]
    assert any("candidate_outputs.npz" in note for note in spec["notes"])
    assert proof_queue._proof_command_policy_error(command) is None


def test_proof_queue_r6_target_version_parity_is_queue_native() -> None:
    spec = proof_queue._r6_target_version_parity_spec("3.12")

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
    assert "tools/target_python_runtime.py" in spec["scopes"]
    assert "src/molt/stdlib/_sys_impl.py" in spec["scopes"]
    assert "src/molt/stdlib/queue.py" in spec["scopes"]
    assert any("serial fail-fast differential custody" in note for note in spec["notes"])
    assert any("missing target interpreters" in note for note in spec["notes"])
    assert proof_queue._proof_command_policy_error(command) is None


def test_proof_queue_r6_target_version_parity_uses_target_tag() -> None:
    spec = proof_queue._r6_target_version_parity_spec("3.13")
    command = list(spec["command"])

    assert spec["logical_id"] == "r6-target-version-parity-py313"
    assert spec["contention_key"] == "python:r6-target-version-py313"
    assert command[command.index("--python-version") + 1] == "3.13"


def test_proof_queue_r6_target_version_parity_print_spec(
    tmp_path: Path, capsys: pytest.CaptureFixture[str]
) -> None:
    db = tmp_path / "proof_queue.sqlite3"

    assert (
        proof_queue.main(
            [
                "--db",
                str(db),
                "--logs-root",
                str(tmp_path / "runs"),
                "--repo-root",
                str(proof_queue.ROOT),
                "r6-target-version-parity",
                "--python-version",
                "3.13",
                "--print-spec",
            ]
        )
        == 0
    )
    spec = json.loads(capsys.readouterr().out)
    assert spec["logical_id"] == "r6-target-version-parity-py313"
    assert spec["command"][spec["command"].index("--python-version") + 1] == "3.13"
    assert "--fail-fast" in spec["command"]
    assert spec["resource_family"] == "python"


def test_proof_queue_pact_witness_acceptance_admits_staged_native_roots(
    tmp_path: Path,
) -> None:
    expected_roots = [
        tmp_path / "tmp/pact_numpy_multiarray_sealed_for_witness",
        tmp_path / "tmp/pact_scipy_ndimage_sealed_for_witness_next",
        tmp_path / "tmp/pact_scipy_ni_label_molt_ext_wasm_cpython_abi",
        tmp_path / "bench/friends/repos/numpy_off_the_shelf",
        tmp_path / "bench/friends/repos/scipy_off_the_shelf",
    ]
    stale_roots = [
        tmp_path / "tmp/pact_numpy_multiarray_sealed_axiserror",
        tmp_path / "tmp/pact_scipy_ndimage_provider_sealed_support_closure",
        tmp_path / "tmp/pact_scipy_ndimage_provider_sealed_helpers",
    ]
    for root in expected_roots:
        root.mkdir(parents=True)
    for root in stale_roots:
        root.mkdir(parents=True)
    for root in [*expected_roots[:3], *stale_roots]:
        (root / "extension_manifest.json").write_text("{}", encoding="utf-8")

    spec = proof_queue._pact_witness_acceptance_spec(repo_root=tmp_path)
    env = spec["env_overrides"]

    assert env["MOLT_EXTERNAL_STATIC_PACKAGES"] == "numpy scipy"
    assert env["MOLT_MODULE_ROOTS"].split(os.pathsep) == [
        str(root.resolve()) for root in expected_roots
    ]
    assert any("manifest-led" in note for note in spec["notes"])


def test_proof_queue_pact_witness_roots_accept_artifact_specific_manifests(
    tmp_path: Path,
) -> None:
    artifact_root = tmp_path / "tmp/pact_scipy_ndimage_sealed_for_witness_next"
    artifact_root.joinpath("scipy", "ndimage").mkdir(parents=True)
    artifact_root.joinpath(
        "scipy", "ndimage", "_nd_image.molt.wasm.extension_manifest.json"
    ).write_text("{}", encoding="utf-8")
    source_roots = [
        tmp_path / "bench/friends/repos/numpy_off_the_shelf",
        tmp_path / "bench/friends/repos/scipy_off_the_shelf",
    ]
    for root in source_roots:
        root.mkdir(parents=True)

    roots = proof_queue._pact_witness_native_roots(repo_root=tmp_path)

    assert roots == [
        artifact_root.resolve(),
        *(root.resolve() for root in source_roots),
    ]


def test_proof_queue_pact_witness_oracle_regenerates_parity_fixture() -> None:
    spec = proof_queue._pact_witness_oracle_spec()

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
    assert "numpy==1.26.4" in command
    assert "scipy==1.17.1" in command
    assert command[-2:] == ["python", "tools/pact_witness_oracle.py"]
    assert "collab/pact/pact_witness_kernel/make_fixture.py" in spec["scopes"]
    assert proof_queue._proof_command_policy_error(command) is None
