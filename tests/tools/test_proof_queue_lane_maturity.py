import argparse
import json
import sqlite3
import sys
from pathlib import Path
from typing import Any

import pytest

from tools.proof_queue_pkg import evidence, runner, scheduling, state


def test_queue_submission_refuses_expensive_wasm_below_l1(tmp_path, capsys):
    source = tmp_path / "source"
    source.mkdir()
    args = argparse.Namespace(
        db=str(tmp_path / "queue.db"),
        logs_root=str(tmp_path / "logs"),
        notebooks_root=str(tmp_path / "notebooks"),
        repo_root=str(source),
    )
    rc, run_id = runner._queue_one(
        args,
        logical_id="witness-lane",
        reason="teeth",
        command=["python", "-c", "print(1)"],
        resource_family="wasm-browser",
        contention_key="wasm:test",
        scopes=[],
        env_overrides={},
        initial_notes=["known-bad maturity fixture"],
    )
    assert rc == 2 and run_id is None
    assert "wasm-browser proof requires L1, lane is L0" in capsys.readouterr().err


def test_queue_submission_allows_wasm_at_l1(tmp_path):
    source = tmp_path / "source"
    source.mkdir()
    scheduling.lane_maturity.write_registry(
        source,
        {
            "witness-lane": {
                "maturity": "L1",
                "status": "active",
                "worktree": str(source),
            }
        },
    )
    args = argparse.Namespace(
        db=str(tmp_path / "queue.db"),
        logs_root=str(tmp_path / "logs"),
        notebooks_root=str(tmp_path / "notebooks"),
        repo_root=str(source),
    )
    rc, run_id = runner._queue_one(
        args,
        logical_id="witness-lane",
        reason="teeth",
        command=["python", "-c", "print(1)"],
        resource_family="wasm-browser",
        contention_key="wasm:test",
        scopes=[],
        env_overrides={},
        initial_notes=["L1 fixture"],
    )
    assert rc == 0 and run_id


def _context(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch, maturity: str = "L0"
) -> tuple[argparse.Namespace, sqlite3.Connection]:
    source = tmp_path / "source"
    source.mkdir()
    scheduling.lane_maturity.write_registry(
        source,
        {
            "child": {
                "maturity": maturity,
                "status": "active",
                "worktree": str(source),
            }
        },
    )
    args = argparse.Namespace(
        db=str(tmp_path / "queue.db"),
        logs_root=str(tmp_path / "logs"),
        notebooks_root=str(tmp_path / "notebooks"),
        repo_root=str(source),
    )
    # Projection is independent of maturity; do not generate a notebook for
    # every disposable metadata-only admission observation.
    monkeypatch.setattr(evidence, "_try_write_marimo_notebook", lambda *a, **k: None)
    return args, state._connect(Path(args.db))


def _insert_run(
    args: argparse.Namespace,
    conn: sqlite3.Connection,
    run_id: str,
    logical_id: str,
    status: str = "passed",
) -> None:
    scheduling._insert_run(
        conn,
        run_id=run_id,
        logical_id=logical_id,
        reason="independent edge-kind maturity fixture",
        command=[sys.executable, "-c", "raise AssertionError('must not execute')"],
        cwd=Path(args.repo_root),
        resource_family="rust",
        contention_key=f"rust:{run_id}",
        scopes=[],
        git_snapshot={"available": False},
        log_path=Path(args.logs_root) / f"{run_id}.log",
        summary_json=Path(args.logs_root) / f"{run_id}.memory_guard.json",
    )
    state._update_run(conn, run_id, status=status)


def _submit(
    args: argparse.Namespace,
    mode: str,
    *,
    parents: list[str],
    kind: str,
    existing_run_id: str | None = None,
) -> tuple[int, str | None]:
    parameters: dict[str, Any] = {
        "logical_id": "child",
        "reason": "independent maturity boundary observation",
        "command": [sys.executable, "-c", "raise AssertionError('must not execute')"],
        "resource_family": "rust",
        "contention_key": "rust:child",
        "scopes": [],
        "env_overrides": {},
        "initial_notes": ["metadata-only fixture; no command launch"],
        "depends_on": parents,
        "edge_kind": kind,
    }
    if mode == "queue":
        return runner._queue_one(args, **parameters)
    rc = runner._run_one(
        args,
        **parameters,
        timeout=1,
        existing_run_id=existing_run_id,
        policy_error="maturity fixture stops before command execution",
    )
    conn = state._connect(Path(args.db))
    row = conn.execute(
        "SELECT run_id FROM proof_runs WHERE logical_id = 'child' AND run_id != 'parent'"
    ).fetchone()
    conn.close()
    return rc, str(row[0]) if row is not None else None


@pytest.mark.parametrize("kind", ["derives_from", "reruns", "supersedes", "compares"])
@pytest.mark.parametrize("mode", ["queue", "direct"])
def test_lineage_submission_does_not_require_cross_lane_artifact_maturity(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch, kind: str, mode: str
) -> None:
    args, conn = _context(tmp_path, monkeypatch)
    _insert_run(args, conn, "parent", "other-lane", "failed")
    rc, run_id = _submit(args, mode, parents=["parent"], kind=kind)
    assert run_id is not None
    assert rc == (0 if mode == "queue" else 2)
    assert [
        tuple(row)
        for row in conn.execute(
            "SELECT parent_run_id, kind FROM proof_run_edges WHERE child_run_id = ?",
            (run_id,),
        )
    ] == [("parent", kind)]
    if mode == "direct":
        row = state._row_by_run_id(conn, run_id)
        assert row is not None
        context = json.loads(row["receipt_context_json"])
        assert context["phase"] == "command policy rejection"
    conn.close()


@pytest.mark.parametrize("mode", ["queue", "direct"])
def test_lineage_submission_can_retain_external_parent_ids(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch, mode: str
) -> None:
    args, conn = _context(tmp_path, monkeypatch)
    rc, run_id = _submit(args, mode, parents=["external-run"], kind="derives_from")
    assert run_id is not None
    assert rc == (0 if mode == "queue" else 2)
    row = conn.execute(
        "SELECT parent_run_id, kind FROM proof_run_edges WHERE child_run_id = ?",
        (run_id,),
    ).fetchone()
    assert tuple(row) == ("external-run", "derives_from")
    conn.close()


@pytest.mark.parametrize("maturity", ["L0", "L3", "L4"])
@pytest.mark.parametrize("mode", ["queue", "direct"])
def test_true_cross_lane_dependency_requires_l4_on_both_submission_paths(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch, maturity: str, mode: str
) -> None:
    args, conn = _context(tmp_path, monkeypatch, maturity)
    _insert_run(args, conn, "parent", "other-lane")
    rc, run_id = _submit(args, mode, parents=["parent"], kind="depends_on")
    assert (run_id is not None) == (maturity == "L4")
    assert rc == (0 if maturity == "L4" and mode == "queue" else 2)
    conn.close()


@pytest.mark.parametrize("mode", ["queue", "direct"])
def test_same_lane_dependency_does_not_require_cross_lane_maturity(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch, mode: str
) -> None:
    args, conn = _context(tmp_path, monkeypatch)
    _insert_run(args, conn, "parent", "child")
    rc, run_id = _submit(args, mode, parents=["parent"], kind="depends_on")
    assert run_id is not None
    assert rc == (0 if mode == "queue" else 2)
    conn.close()


@pytest.mark.parametrize("mode", ["queue", "direct"])
def test_scheduling_dependency_still_requires_a_local_parent(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch, mode: str
) -> None:
    args, conn = _context(tmp_path, monkeypatch, "L4")
    with pytest.raises(SystemExit, match="unknown parent proof run 'external-run'"):
        _submit(args, mode, parents=["external-run"], kind="depends_on")
    assert conn.execute("SELECT COUNT(*) FROM proof_runs").fetchone()[0] == 0
    conn.close()


@pytest.mark.parametrize("mode", ["queue", "direct"])
def test_unknown_edge_kind_cannot_bypass_maturity_as_lineage(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch, mode: str
) -> None:
    args, conn = _context(tmp_path, monkeypatch)
    with pytest.raises(SystemExit, match="unknown proof edge kind 'invented'"):
        _submit(args, mode, parents=[], kind="invented")
    assert conn.execute("SELECT COUNT(*) FROM proof_runs").fetchone()[0] == 0
    conn.close()


@pytest.mark.parametrize("has_dependency", [False, True])
@pytest.mark.parametrize("maturity", ["L0", "L3", "L4"])
def test_queued_replay_uses_only_persisted_scheduling_edges_for_maturity(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
    has_dependency: bool,
    maturity: str,
) -> None:
    args, conn = _context(tmp_path, monkeypatch, maturity)
    _insert_run(args, conn, "failed-parent", "other-lineage-lane", "failed")
    _insert_run(args, conn, "dependency", "artifact-lane")
    _insert_run(args, conn, "queued-child", "child", "queued")
    for kind in ["derives_from", "reruns", "supersedes", "compares"]:
        state._insert_edge(
            conn,
            parent_run_id="failed-parent",
            child_run_id="queued-child",
            kind=kind,
        )
    if has_dependency:
        state._insert_edge(
            conn,
            parent_run_id="dependency",
            child_run_id="queued-child",
            kind="depends_on",
        )
    # Queued execution supplies no parent arguments. The stored DAG remains
    # authoritative even if a caller labels the empty incoming batch lineage.
    rc, _ = _submit(
        args, "direct", parents=[], kind="derives_from", existing_run_id="queued-child"
    )
    assert rc == 2
    row = state._row_by_run_id(conn, "queued-child")
    assert row is not None
    admitted = not has_dependency or maturity == "L4"
    assert row["status"] == ("failed" if admitted else "queued")
    if admitted:
        assert (
            json.loads(row["receipt_context_json"])["phase"]
            == "command policy rejection"
        )
    dependency_state, blockers = scheduling._dependency_state(conn, "queued-child")
    assert dependency_state == "ready" and blockers == []
    conn.close()


def test_lineage_does_not_remove_the_wasm_resource_family_threshold(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    args, conn = _context(tmp_path, monkeypatch)
    _insert_run(args, conn, "parent", "other-lane", "failed")
    decision = scheduling._lane_maturity_admission(
        conn=conn,
        repo_root=Path(args.repo_root),
        logical_id="child",
        resource_family="wasm",
        depends_on=["parent"],
        edge_kind="compares",
    )
    assert not decision.allow and decision.required == "L1"
    conn.close()
