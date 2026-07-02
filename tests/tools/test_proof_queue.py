from __future__ import annotations

import json
import os
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


def _edges(db: Path) -> list[sqlite3.Row]:
    conn = sqlite3.connect(db)
    conn.row_factory = sqlite3.Row
    return list(conn.execute("SELECT * FROM proof_run_edges ORDER BY edge_id"))


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
    assert '"note_kind_counts": {' in notebook_text
    assert '"submission": 1' in notebook_text


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
        proof_queue.main(
            [*base_args, "evidence", run_id, "--run-id", "not-a-run-id"]
        )


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
            "pyinit_module_to_bits_reports_static_link_py_mod_exec_pending_error",
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
    assert command[14:17] == [
        "-p",
        "molt-runtime",
        "pyinit_module_to_bits_reports_static_link_py_mod_exec_pending_error",
    ]
    assert command[-1] == "--lib"
    assert [note["body"] for note in _notes(db)] == ["canonical cargo proof lane smoke"]


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
                'edge_kind = "derives_from"',
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
    assert edges[0]["kind"] == "derives_from"
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
    assert '"derives_from": 1' in notebook_text


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
        kind="reruns",
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


def test_proof_queue_diagnoses_runtime_wasm_missing_required_exports(
    tmp_path: Path, capsys: pytest.CaptureFixture[str]
) -> None:
    db = tmp_path / "proof_queue.sqlite3"
    log_path = tmp_path / "runtime-missing-exports.log"
    conn = proof_queue._connect(db)
    proof_queue._insert_run(
        conn,
        run_id="runtime-missing-exports",
        logical_id="pact-witness-acceptance",
        reason="prove runtime export diagnostics",
        command=[sys.executable, "-m", "molt", "build", "field_solve.py"],
        cwd=proof_queue.ROOT,
        resource_family="wasm",
        contention_key="wasm:pact-witness",
        scopes=["src/molt/cli/runtime_build.py"],
        git_snapshot={
            "available": True,
            "head": "abc123",
            "dirty": False,
            "status": [],
        },
        log_path=log_path,
        summary_json=tmp_path / "runtime-missing-exports.memory_guard.json",
    )
    log_path.write_text(
        "Runtime wasm build produced artifact missing required exports: "
        "PyArg_ParseTuple, PyErr_Format, PyObject_CallFunction\n",
        encoding="utf-8",
    )
    proof_queue._update_run(
        conn,
        "runtime-missing-exports",
        status="failed",
        returncode=1,
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
                "runtime-missing-exports",
            ]
        )
        == 0
    )
    evidence = json.loads(capsys.readouterr().out)
    diagnostics = evidence[0]["diagnostics"]
    assert diagnostics[0]["signal_id"] == "runtime-wasm-missing-required-exports"
    assert "PyArg_ParseTuple" in diagnostics[0]["summary"]
    assert "wasm_runtime_shared_export_link_args" in diagnostics[0]["next_action"]


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
