from __future__ import annotations

import sqlite3
import sys
from pathlib import Path

import pytest

import tools.proof_queue as proof_queue


def _rows(db: Path) -> list[sqlite3.Row]:
    conn = sqlite3.connect(db)
    conn.row_factory = sqlite3.Row
    return list(conn.execute("SELECT * FROM proof_runs ORDER BY rowid"))


def test_proof_queue_session_id_is_contention_key_scoped() -> None:
    assert proof_queue._proof_session_id(
        "wasm", "wasm-build"
    ) == proof_queue._proof_session_id("wasm", "wasm-build")
    assert proof_queue._proof_session_id(
        "wasm", "wasm-build"
    ) != proof_queue._proof_session_id("wasm", "wasm-browser")


def test_proof_queue_exec_records_passed_run(tmp_path: Path) -> None:
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
            "queue-smoke",
            "--reason",
            "prove queue smoke",
            "--resource-family",
            "python",
            "--contention-key",
            "python:queue-smoke",
            "--env",
            "PROOF_QUEUE_TEST=queue-ok",
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
    assert "queue-ok" in Path(rows[0]["log_path"]).read_text(encoding="utf-8")


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
