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


def _notes(db: Path) -> list[sqlite3.Row]:
    conn = sqlite3.connect(db)
    conn.row_factory = sqlite3.Row
    return list(conn.execute("SELECT * FROM proof_notes ORDER BY note_id"))


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
    notebooks = tmp_path / "notebooks"

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
    assert "queue-ok" in Path(rows[0]["log_path"]).read_text(encoding="utf-8")
    notes = _notes(db)
    assert [note["body"] for note in notes] == [
        "changed queue smoke to verify note capture"
    ]
    notebook = notebooks / f"{rows[0]['run_id']}.py"
    notebook_text = notebook.read_text(encoding="utf-8")
    assert "import marimo" in notebook_text
    assert '"status": "passed"' in notebook_text
    assert "changed queue smoke to verify note capture" in notebook_text


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
    assert '"notes": [' in payload
    assert '"head": "abc123"' in payload
    assert "R18 is still running" in payload


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
    assert command[7:10] == ["python", "-m", "molt"]
    assert "build" in command
    assert "collab/pact/pact_witness_kernel/field_solve.py" in command
    assert "--split-runtime" in command
    assert "tmp/pact_witness_acceptance_queue" in command
    assert "collab/pact/pact_witness_kernel/check_parity.py" in spec["scopes"]
    assert proof_queue._proof_command_policy_error(command) is None


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
