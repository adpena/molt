"""Queue-owned run evidence retention on real temporary queue roots.

Each test builds rows and files, states the expected kept and reclaimed sets
from the retention rule, and checks the filesystem and raw SQLite directly.
"""

from __future__ import annotations

from contextlib import closing
from dataclasses import dataclass
import hashlib
import json
import os
from pathlib import Path
import sqlite3

import pytest

from molt.exact_json import write_exact
from molt.temporary_artifacts import ScratchRetention
from tools.proof_queue_pkg import (
    cargo_cache_custody,
    cli,
    run_retention,
    scheduling,
    state,
)


@dataclass(frozen=True)
class Queue:
    root: Path
    db: Path
    runs: Path
    notebooks: Path
    source: Path

    def argv(self, *args: str) -> list[str]:
        return [
            "--db",
            str(self.db),
            "--logs-root",
            str(self.runs),
            "--notebooks-root",
            str(self.notebooks),
            "--repo-root",
            str(self.source),
            *args,
        ]


@pytest.fixture
def queue(tmp_path: Path) -> Queue:
    root = tmp_path.resolve()
    made = Queue(
        root=root,
        db=root / "queue.sqlite3",
        runs=root / "runs",
        notebooks=root / "notebooks",
        source=root / "source",
    )
    made.runs.mkdir()
    made.source.mkdir()
    return made


def _finished(order: int) -> str:
    return f"2026-10-01T00:{order // 60:02d}:{order % 60:02d}+00:00"


def _nonce(run_id: str) -> str:
    return hashlib.sha256(run_id.encode()).hexdigest()


def add_run(
    queue: Queue,
    run_id: str,
    *,
    status: str = "passed",
    order: int = 0,
    log_bytes: int = 64,
    extra: tuple[str, ...] = (),
    scratch_bytes: int | None = None,
    request_scratch: bool = False,
    peak_rss_kb: int | None = None,
    cargo_target: Path | None = None,
    log_dir: Path | None = None,
) -> None:
    """Insert one row and write the files the runner writes for it."""
    directory = log_dir or queue.runs
    directory.mkdir(parents=True, exist_ok=True)
    log_path = directory / f"{run_id}.log"
    summary = directory / f"{run_id}.memory_guard.json"
    with closing(state._connect(queue.db)) as conn:
        scheduling._insert_run(
            conn,
            run_id=run_id,
            logical_id=run_id,
            reason="retention fixture",
            command=["cargo", "test"],
            cwd=queue.source,
            resource_family="python",
            contention_key=f"python:{run_id}",
            scopes=["tests"],
            git_snapshot={},
            log_path=log_path,
            summary_json=summary,
        )
        log_path.write_bytes(b"L" * log_bytes)
        summary_payload: dict[str, object] = {"phase": "final"}
        if peak_rss_kb is not None:
            summary_payload["peak"] = {"rss_kb": peak_rss_kb}
        summary.write_text(json.dumps(summary_payload), encoding="utf-8")
        for suffix in extra:
            (directory / f"{run_id}{suffix}").write_bytes(b"E" * 16)
        context: dict[str, object] = dict(
            state._unattested_receipt_context(
                status="not-executed", phase="fixture", reason="fixture"
            )
        )
        prelaunch: list[dict[str, object]] = []
        nonce = _nonce(run_id)
        scratch = queue.runs / "derived" / nonce / "scratch"
        if scratch_bytes is not None:
            scratch.mkdir(parents=True)
            (scratch / "out.bin").write_bytes(b"S" * scratch_bytes)
        if scratch_bytes is not None and not request_scratch:
            prelaunch.append(
                {"role": "scratch-output", "path": str(scratch), "run_owned": True}
            )
        if request_scratch:
            write_exact(
                log_path.with_suffix(".execution-request.json"),
                {"run_id": run_id, "execution_nonce": nonce},
            )
        if cargo_target is not None:
            prelaunch.append(
                {"schema": cargo_cache_custody.SCHEMA, "path": str(cargo_target)}
            )
        if prelaunch or peak_rss_kb is not None:
            # An attested-shaped context that evidence lookups can project.
            context = {
                "environment": {"os": "fixture-os", "arch": "fixture-arch"},
                "execution_nonce_sha256": hashlib.sha256(nonce.encode()).hexdigest(),
                "derived_root_custody": {"prelaunch": prelaunch},
            }
        if status != "queued":
            state._update_run(
                conn,
                run_id,
                status=status,
                returncode=0 if status == "passed" else 2,
                started_at=_finished(order),
                finished_at=_finished(order),
                elapsed_s=1.0,
                receipt_context_json=json.dumps(context, sort_keys=True),
            )


def snapshot(root: Path) -> dict[str, str]:
    """Content of every path under root, except the queue database files."""
    found: dict[str, str] = {}
    for path in sorted(root.rglob("*")):
        relative = path.relative_to(root).as_posix()
        if relative.startswith("queue.sqlite3"):
            continue
        found[relative] = (
            "<dir>" if path.is_dir() else hashlib.sha256(path.read_bytes()).hexdigest()
        )
    return found


def dump(db: Path) -> list[str]:
    with closing(sqlite3.connect(db)) as conn:
        return list(conn.iterdump())


def retention_states(db: Path) -> dict[str, str | None]:
    with closing(sqlite3.connect(db)) as conn:
        return {
            str(run_id): state_
            for run_id, state_ in conn.execute(
                "SELECT run_id, json_extract(evidence_retention_json, '$.state') "
                "FROM proof_runs"
            )
        }


def owned_paths(queue: Queue, run_id: str) -> set[str]:
    """Every path a run owns, named independently of the retention module."""
    owned = {
        path.relative_to(queue.root).as_posix()
        for path in queue.runs.iterdir()
        if path.name.split(".", 1)[0] == run_id
    }
    derived = queue.runs / "derived" / _nonce(run_id)
    if derived.exists():
        owned |= {
            path.relative_to(queue.root).as_posix()
            for path in (derived, *derived.rglob("*"))
        }
    return owned


def run_cli(
    queue: Queue, capsys: pytest.CaptureFixture[str], *args: str
) -> tuple[int, str]:
    capsys.readouterr()
    rc = cli.main(queue.argv(*args))
    return rc, capsys.readouterr().out


def build_mixed_queue(queue: Queue) -> dict[str, str]:
    """One run per class. Returns run id -> expected disposition at --keep-runs 2."""
    live_target = queue.root / "cargo-live" / "target"
    live_target.mkdir(parents=True)
    add_run(queue, "failed-1", status="failed", extra=(".runner.log",))
    add_run(queue, "stale-1", status="stale")
    add_run(queue, "parent-0", status="passed")
    add_run(queue, "queued-1", status="queued")
    add_run(queue, "pinned-1", status="passed")
    add_run(queue, "cargo-1", status="passed", cargo_target=live_target)
    add_run(queue, "nonevidence-1", status="non-evidence", order=1, scratch_bytes=512)
    add_run(queue, "blocked-1", status="blocked", order=2)
    add_run(queue, "passed-1", status="passed", order=3, extra=(".execution.json",))
    add_run(
        queue,
        "passed-2",
        status="passed",
        order=4,
        scratch_bytes=256,
        request_scratch=True,
    )
    add_run(queue, "passed-3", status="passed", order=5)
    add_run(queue, "passed-4", status="passed", order=6)
    add_run(queue, "foreign-1", status="passed", order=7, log_dir=queue.root / "other")
    with closing(state._connect(queue.db)) as conn:
        state._insert_edge(conn, parent_run_id="parent-0", child_run_id="queued-1")
        state._insert_note(
            conn, run_id="pinned-1", body="cited by a fixture", kind="retain"
        )
    (queue.runs / "stray.log").write_bytes(b"x" * 10)
    return {
        "failed-1": "failed",
        "stale-1": "unresolved",
        "queued-1": "unresolved",
        "parent-0": "active-dependency",
        "pinned-1": "pinned",
        "cargo-1": "cargo-generation-live",
        "foreign-1": "custody-unverified",
        "passed-4": "window",
        "passed-3": "window",
        "passed-2": "reclaim",
        "passed-1": "reclaim",
        "blocked-1": "reclaim",
        "nonevidence-1": "reclaim",
    }


def test_dry_run_reports_every_class_and_changes_nothing(
    queue: Queue, capsys: pytest.CaptureFixture[str]
) -> None:
    expected = build_mixed_queue(queue)
    before_tree = snapshot(queue.root)
    before_db = dump(queue.db)

    rc, out = run_cli(queue, capsys, "retention", "--keep-runs", "2", "--json")

    assert rc == 0
    report = json.loads(out)
    assert report["apply"] is False
    reclaim = {item["run_id"] for item in report["reclaim"]}
    assert reclaim == {run for run, kind in expected.items() if kind == "reclaim"}
    counts: dict[str, int] = {}
    for kind in expected.values():
        counts[kind] = counts.get(kind, 0) + 1
    assert {name: value["runs"] for name, value in report["classes"].items()} == counts
    reclaim_bytes = {item["run_id"]: item["bytes"] for item in report["reclaim"]}
    # The scratch of a run found through its execution request counts too.
    assert reclaim_bytes["passed-2"] > 256
    assert reclaim_bytes["nonevidence-1"] > 512
    assert report["unowned_files"] == {"files": 1, "bytes": 10}
    assert snapshot(queue.root) == before_tree
    assert dump(queue.db) == before_db

    rc, human = run_cli(queue, capsys, "retention", "--keep-runs", "2")
    assert rc == 0
    assert "dry run; add --apply to reclaim" in human
    assert "would reclaim: 4 runs" in human
    assert snapshot(queue.root) == before_tree


def test_apply_reclaims_outside_the_window_and_keeps_protected_runs(
    queue: Queue, capsys: pytest.CaptureFixture[str]
) -> None:
    expected = build_mixed_queue(queue)
    reclaim = {run for run, kind in expected.items() if kind == "reclaim"}
    doomed = set().union(*(owned_paths(queue, run) for run in reclaim))
    assert any("derived" in path for path in doomed)
    before = snapshot(queue.root)
    with closing(sqlite3.connect(queue.db)) as conn:
        notes_before = conn.execute("SELECT COUNT(*) FROM proof_notes").fetchone()
        edges_before = conn.execute("SELECT COUNT(*) FROM proof_run_edges").fetchone()

    rc, out = run_cli(
        queue, capsys, "retention", "--keep-runs", "2", "--apply", "--json"
    )

    assert rc == 0
    report = json.loads(out)
    assert {item["run_id"] for item in report["reclaimed"]} == reclaim
    after = snapshot(queue.root)
    assert after == {
        path: digest for path, digest in before.items() if path not in doomed
    }
    states = retention_states(queue.db)
    assert {run for run, value in states.items() if value == "reclaimed"} == reclaim
    assert all(states[run] is None for run in expected if run not in reclaim)
    with closing(sqlite3.connect(queue.db)) as conn:
        assert conn.execute("SELECT COUNT(*) FROM proof_runs").fetchone()[0] == len(
            expected
        )
        assert conn.execute("SELECT COUNT(*) FROM proof_notes").fetchone() == (
            notes_before
        )
        assert (
            conn.execute("SELECT COUNT(*) FROM proof_run_edges").fetchone()
            == edges_before
        )

    rc, out = run_cli(
        queue, capsys, "retention", "--keep-runs", "2", "--apply", "--json"
    )
    assert rc == 0
    assert json.loads(out)["reclaimed"] == []
    assert snapshot(queue.root) == after


def _sized_runs(queue: Queue, sizes: list[int]) -> list[str]:
    run_ids = []
    for order, size in enumerate(sizes):
        run_id = f"sized-{order}"
        add_run(queue, run_id, order=order, log_bytes=size)
        (queue.runs / f"{run_id}.memory_guard.json").unlink()
        run_ids.append(run_id)
    return run_ids


def test_byte_budget_keeps_the_newest_runs_that_fit(queue: Queue) -> None:
    # Oldest first. Newest first the rule keeps 300, 100, skips 5000, keeps
    # 100 and 100: 600 bytes in all.
    runs = _sized_runs(queue, [100, 100, 5000, 100, 300])
    report = run_retention.run_pass(
        db=queue.db,
        result_root=queue.runs,
        policy=ScratchRetention(count=10, bytes=600),
        apply=True,
    )
    assert [item["run_id"] for item in report["reclaimed"]] == [runs[2]]
    assert sorted(path.name for path in queue.runs.iterdir()) == sorted(
        f"{run}.log" for run in runs if run != runs[2]
    )


def test_the_newest_run_stays_even_when_it_exceeds_the_budget(queue: Queue) -> None:
    runs = _sized_runs(queue, [10, 10, 10_000])
    report = run_retention.run_pass(
        db=queue.db,
        result_root=queue.runs,
        policy=ScratchRetention(count=10, bytes=600),
        apply=True,
    )
    assert {item["run_id"] for item in report["reclaimed"]} == set(runs[:2])
    assert [path.name for path in queue.runs.iterdir()] == [f"{runs[2]}.log"]


class Interrupted(BaseException):
    """Stands for a killed process between two deletions."""


def test_interrupted_reclaim_is_finished_by_the_next_pass(
    queue: Queue, capsys: pytest.CaptureFixture[str], monkeypatch: pytest.MonkeyPatch
) -> None:
    add_run(
        queue,
        "old",
        status="non-evidence",
        order=1,
        extra=(".runner.log",),
        scratch_bytes=128,
        peak_rss_kb=4,
    )
    add_run(queue, "new", order=2)
    owned = owned_paths(queue, "old")
    real_delete = run_retention.delete_path
    calls: list[Path] = []

    def interrupt_second(path: Path) -> tuple[bool, str]:
        calls.append(path)
        if len(calls) == 2:
            raise Interrupted
        return real_delete(path)

    monkeypatch.setattr(run_retention, "delete_path", interrupt_second)
    with pytest.raises(Interrupted):
        run_cli(queue, capsys, "retention", "--keep-runs", "1", "--apply")
    assert retention_states(queue.db) == {"old": "reclaiming", "new": None}
    remaining = owned_paths(queue, "old")
    assert remaining and remaining < owned

    # Lookups already know: they read the claim, never the half-deleted files.
    rc, out = run_cli(queue, capsys, "evidence", "--run-id", "old")
    assert rc == 0
    payload = json.loads(out)[0]
    assert payload["evidence_retention"]["state"] == "reclaiming"
    assert [item["signal_id"] for item in payload["diagnostics"]] == [
        "proof-evidence-reclaimed"
    ]
    assert payload["proof_receipt"]["commands"][0]["peak_rss_bytes"] == 4 * 1024

    monkeypatch.setattr(run_retention, "delete_path", real_delete)
    rc, out = run_cli(
        queue, capsys, "retention", "--keep-runs", "1", "--apply", "--json"
    )
    assert rc == 0
    report = json.loads(out)
    assert report["recovered"] == ["old"]
    assert report["reclaimed"] == []
    assert owned_paths(queue, "old") == set()
    assert retention_states(queue.db) == {"old": "reclaimed", "new": None}
    assert (queue.runs / "new.log").is_file()


def test_failed_deletion_stays_claimed_and_the_next_pass_retries(
    queue: Queue, capsys: pytest.CaptureFixture[str], monkeypatch: pytest.MonkeyPatch
) -> None:
    add_run(queue, "old", status="non-evidence", order=1, scratch_bytes=64)
    add_run(queue, "new", order=2)
    real_delete = run_retention.delete_path

    def refuse_scratch(path: Path) -> tuple[bool, str]:
        if path.parent.name == "derived":
            return False, "fixture refusal"
        return real_delete(path)

    monkeypatch.setattr(run_retention, "delete_path", refuse_scratch)
    rc, out = run_cli(
        queue, capsys, "retention", "--keep-runs", "1", "--apply", "--json"
    )
    assert rc == 2
    assert any("fixture refusal" in error for error in json.loads(out)["errors"])
    with closing(sqlite3.connect(queue.db)) as conn:
        record = json.loads(
            conn.execute(
                "SELECT evidence_retention_json FROM proof_runs WHERE run_id = 'old'"
            ).fetchone()[0]
        )
    assert record["state"] == "reclaiming"
    assert "fixture refusal" in record["error"]
    assert (queue.runs / "derived" / _nonce("old")).is_dir()

    monkeypatch.setattr(run_retention, "delete_path", real_delete)
    rc, out = run_cli(
        queue, capsys, "retention", "--keep-runs", "1", "--apply", "--json"
    )
    assert rc == 0
    assert json.loads(out)["recovered"] == ["old"]
    assert owned_paths(queue, "old") == set()


def test_evidence_status_and_audit_read_the_reclaimed_record(
    queue: Queue, capsys: pytest.CaptureFixture[str]
) -> None:
    add_run(queue, "old", status="non-evidence", order=1, peak_rss_kb=2048)
    add_run(queue, "new", order=2)
    rc, out = run_cli(queue, capsys, "evidence", "--run-id", "old")
    assert rc == 0
    before = json.loads(out)[0]
    assert before["proof_receipt"]["commands"][0]["peak_rss_bytes"] == 2048 * 1024
    assert "evidence_retention" not in before

    rc, _ = run_cli(queue, capsys, "retention", "--keep-runs", "1", "--apply")
    assert rc == 0
    assert not (queue.runs / "old.log").exists()

    rc, out = run_cli(queue, capsys, "evidence", "--run-id", "old")
    assert rc == 0
    after = json.loads(out)[0]
    assert after["evidence_retention"]["state"] == "reclaimed"
    assert after["evidence_retention"]["paths"] == sorted(
        [str(queue.runs / "old.log"), str(queue.runs / "old.memory_guard.json")]
    )
    # The receipt is unchanged: retention saved peak RSS before the summary went.
    assert after["proof_receipt"] == before["proof_receipt"]
    assert after["log_path"] == before["log_path"]
    assert [item["signal_id"] for item in after["diagnostics"]] == [
        "proof-evidence-reclaimed"
    ]

    rc, out = run_cli(queue, capsys, "status")
    assert rc == 0
    assert "diagnosis=proof-evidence-reclaimed [operator]" in out

    # Other fixture rows carry unattested receipts, so audit may still fail;
    # the reclaimed row itself must raise no error.
    _rc, out = run_cli(queue, capsys, "audit", "--all", "--json", "--no-notebook-check")
    issues = json.loads(out)["issues"]
    assert not [
        issue
        for issue in issues
        if issue["run_id"] == "old" and issue["severity"] == "error"
    ]

    with pytest.raises(SystemExit, match="already reclaimed"):
        cli.main(queue.argv("note", "old", "--kind", "retain", "--note", "too late"))


@pytest.mark.parametrize("race", ["pin", "unresolved-child"])
def test_the_claim_rechecks_eligibility_in_the_database(
    queue: Queue, monkeypatch: pytest.MonkeyPatch, race: str
) -> None:
    add_run(queue, "old", order=1)
    add_run(queue, "new", order=2)
    real_plan = run_retention.plan

    def plan_then_race(conn: sqlite3.Connection, **kwargs: object):
        result = real_plan(conn, **kwargs)  # type: ignore[arg-type]
        with closing(state._connect(queue.db)) as other:
            if race == "pin":
                state._insert_note(other, run_id="old", body="cited", kind="retain")
            else:
                add_run(queue, "child", status="queued")
                state._insert_edge(other, parent_run_id="old", child_run_id="child")
        return result

    monkeypatch.setattr(run_retention, "plan", plan_then_race)
    report = run_retention.run_pass(
        db=queue.db,
        result_root=queue.runs,
        policy=ScratchRetention(count=1, bytes=1024),
        apply=True,
    )
    assert report["skipped"] == ["old"]
    assert report["reclaimed"] == []
    assert (queue.runs / "old.log").is_file()
    assert retention_states(queue.db)["old"] is None


def test_automatic_pass_reclaims_a_bounded_oldest_batch(
    queue: Queue, monkeypatch: pytest.MonkeyPatch
) -> None:
    runs = [f"auto-{order:02d}" for order in range(41)]
    for order, run_id in enumerate(runs):
        add_run(queue, run_id, order=order)
    monkeypatch.setenv(run_retention.RETAIN_RUNS_ENV, "1")

    first = run_retention.automatic_pass(
        db=queue.db, result_root=queue.runs, env=os.environ
    )
    limit = run_retention.AUTOMATIC_RECLAIM_LIMIT
    assert [item["run_id"] for item in first["reclaimed"]] == runs[:limit]
    second = run_retention.automatic_pass(
        db=queue.db, result_root=queue.runs, env=os.environ
    )
    assert [item["run_id"] for item in second["reclaimed"]] == runs[limit:-1]
    assert sorted(path.name for path in queue.runs.iterdir()) == sorted(
        f"{runs[-1]}{suffix}" for suffix in (".log", ".memory_guard.json")
    )


def test_reclaimed_rows_cannot_be_rewritten(queue: Queue) -> None:
    add_run(queue, "old", order=1)
    add_run(queue, "new", order=2)
    run_retention.run_pass(
        db=queue.db,
        result_root=queue.runs,
        policy=ScratchRetention(count=1, bytes=1024),
        apply=True,
    )
    with closing(state._connect(queue.db)) as conn:
        for statement in (
            "UPDATE proof_runs SET evidence_retention_json = NULL WHERE run_id = 'old'",
            "UPDATE proof_runs SET status = 'failed' WHERE run_id = 'old'",
            "UPDATE proof_runs SET log_path = 'elsewhere' WHERE run_id = 'old'",
        ):
            with pytest.raises(sqlite3.IntegrityError):
                conn.execute(statement)


def test_policy_comes_from_option_then_environment_then_default() -> None:
    assert run_retention.configured_policy({}) == ScratchRetention(
        count=200, bytes=8 * 1024**3
    )
    env = {run_retention.RETAIN_RUNS_ENV: "5", run_retention.RETAIN_GB_ENV: "0.5"}
    assert run_retention.configured_policy(env) == ScratchRetention(
        count=5, bytes=1024**3 // 2
    )
    assert run_retention.configured_policy(env, runs=7, gb=1.0) == ScratchRetention(
        count=7, bytes=1024**3
    )
    for bad in (
        {run_retention.RETAIN_RUNS_ENV: "0"},
        {run_retention.RETAIN_RUNS_ENV: "many"},
        {run_retention.RETAIN_GB_ENV: "-1"},
        {run_retention.RETAIN_GB_ENV: "nan"},
        {run_retention.RETAIN_GB_ENV: "inf"},
    ):
        with pytest.raises(ValueError):
            run_retention.configured_policy(bad)
