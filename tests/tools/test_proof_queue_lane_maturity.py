import argparse

from tools.proof_queue_pkg import runner, scheduling


def test_queue_submission_refuses_expensive_wasm_below_l1(tmp_path):
    args = argparse.Namespace(
        db=str(tmp_path / "queue.db"),
        logs_root=str(tmp_path / "logs"),
        notebooks_root=str(tmp_path / "notebooks"),
        repo_root=str(tmp_path),
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


def test_queue_submission_allows_wasm_at_l1(tmp_path):
    (tmp_path / ".git").mkdir()
    scheduling.lane_maturity.write_registry(
        tmp_path,
        {
            "witness-lane": {
                "maturity": "L1",
                "status": "active",
                "worktree": str(tmp_path),
            }
        },
    )
    args = argparse.Namespace(
        db=str(tmp_path / "queue.db"),
        logs_root=str(tmp_path / "logs"),
        notebooks_root=str(tmp_path / "notebooks"),
        repo_root=str(tmp_path),
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
