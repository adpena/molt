from __future__ import annotations

import json
import os
import sqlite3
import subprocess
from pathlib import Path

import pytest

from molt.dx import CheckoutCustody
from molt.path_custody import CustodyPathRole, PathCustodyError, canonical_host_path
from tests.process_guard_common import run_guarded_test_process
from tools import runtime_wasm_final_preflight as preflight


def _queue_db(path: Path, *, rows: tuple[tuple[object, ...], ...]) -> None:
    with sqlite3.connect(path) as conn:
        conn.execute(
            """
            CREATE TABLE proof_runs (
                run_id TEXT, resource_family TEXT, contention_key TEXT,
                resource_mutex_key TEXT, status TEXT, guard_pid INTEGER,
                started_at TEXT, command_json TEXT
            )
            """
        )
        conn.executemany("INSERT INTO proof_runs VALUES (?, ?, ?, ?, ?, ?, ?, ?)", rows)


def _row(
    run_id: str, *, status: str = "running", guard_pid: int | None = None
) -> tuple[object, ...]:
    return (
        run_id,
        "wasm",
        "wasm:final",
        "compiler-build-resource",
        status,
        os.getpid() if guard_pid is None else guard_pid,
        "2026-07-18T00:00:00Z",
        "[]",
    )


def _context(tmp_path: Path) -> preflight.RuntimeWasmPreflightContext:
    project = tmp_path / "project"
    custody = tmp_path / "custody"
    target = custody / "target"
    cache = custody / "cache"
    runtime = custody / "wasm"
    for path in (project, custody, target, cache, runtime):
        path.mkdir(parents=True, exist_ok=True)
    db = custody / "proof.sqlite3"
    _queue_db(db, rows=(_row("run-current"),))
    claim = preflight._proof_queue_claim(db, "run-current")
    roots = preflight.RuntimeWasmPreflightRoots(
        project=project.resolve(),
        custody=custody.resolve(),
        target=target.resolve(),
        cache=cache.resolve(),
        runtime=runtime.resolve(),
        proof_queue_db=db.resolve(),
        marker_dirs=(),
    )
    return preflight.RuntimeWasmPreflightContext(
        roots=roots,
        claim=claim,
        build_env={
            "CARGO_TARGET_DIR": str(target),
            "MOLT_CACHE": str(cache),
            "MOLT_WASM_RUNTIME_DIR": str(runtime),
        },
    )


def test_active_build_guards_require_explicit_live_marker_custody(
    tmp_path: Path,
) -> None:
    marker_dir = tmp_path / "active"
    marker_dir.mkdir()
    marker = marker_dir / "guard-123-token.json"
    marker.write_text(
        json.dumps(
            {
                "pid": 123,
                "status": "child_running",
                "command": ["python", "-m", "molt.cli", "internal-runtime-wasm-build"],
                "cwd": str(tmp_path),
            }
        ),
        encoding="utf-8",
    )

    assert preflight._active_build_guards((marker_dir,), live_pids=frozenset({123}))
    assert (
        preflight._active_build_guards(
            (marker_dir,), live_pids=frozenset({123}), exclude_pids=frozenset({123})
        )
        == []
    )


def test_proof_queue_claim_is_typed_and_competing_claims_fail_closed(
    tmp_path: Path,
) -> None:
    db = tmp_path / "proof.sqlite3"
    _queue_db(db, rows=(_row("run-current"), _row("run-other")))

    claim = preflight._proof_queue_claim(db, "run-current")
    conflicts = preflight._proof_queue_conflicts(db, exclude_run_id="run-current")

    assert claim.resource_mutex_key == "compiler-build-resource"
    assert [row["run_id"] for row in conflicts] == ["run-other"]


def test_proof_queue_claim_rejects_non_running_row(tmp_path: Path) -> None:
    db = tmp_path / "proof.sqlite3"
    _queue_db(db, rows=(_row("run-current", status="completed"),))

    with pytest.raises(preflight.RuntimeWasmPreflightError, match="not a live"):
        preflight._proof_queue_claim(db, "run-current")


def test_preflight_revalidates_source_disk_pair_and_custody_under_claim(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    context = _context(tmp_path)
    identities = iter(
        (
            {"working_tree_digest": "3" * 64},
            {"working_tree_digest": "3" * 64},
        )
    )
    monkeypatch.setattr(preflight, "_source_identity", lambda _root: next(identities))
    monkeypatch.setattr(
        preflight, "_planned_pair", lambda **_kwargs: {"pair_digest": "ab" * 32}
    )
    from tools import memory_guard

    monkeypatch.setattr(
        memory_guard, "sample_processes", lambda: {os.getpid(): object()}
    )

    payload = preflight.build_preflight(
        context=context,
        reserve_bytes=1,
        build_profile="release",
        stdlib_profile="full",
    )

    assert payload["status"] == "ready"
    assert payload["checks"] == {
        "canonical_exact_roots": True,
        "disk_reserve_revalidated": True,
        "exclusive_build_custody_revalidated": True,
        "compiler_build_claim_held": True,
        "live_guard_custody_binding": True,
        "planned_pair_identity": True,
        "source_identity_revalidated": True,
    }
    assert payload["claim"]["run_id"] == "run-current"


def test_preflight_blocks_when_source_changes_while_claim_is_held(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    context = _context(tmp_path)
    identities = iter(
        (
            {"working_tree_digest": "1" * 64},
            {"working_tree_digest": "2" * 64},
        )
    )
    monkeypatch.setattr(preflight, "_source_identity", lambda _root: next(identities))
    monkeypatch.setattr(
        preflight, "_planned_pair", lambda **_kwargs: {"pair_digest": "ab" * 32}
    )
    from tools import memory_guard

    monkeypatch.setattr(
        memory_guard, "sample_processes", lambda: {os.getpid(): object()}
    )

    payload = preflight.build_preflight(
        context=context,
        reserve_bytes=1,
        build_profile="release",
        stdlib_profile="full",
    )

    assert payload["status"] == "blocked"
    assert payload["checks"]["source_identity_revalidated"] is False


def test_source_identity_uses_unambiguous_length_framing() -> None:
    assert preflight._framed_digest(((b"a", b"bc"),)) != preflight._framed_digest(
        ((b"ab", b"c"),)
    )
    assert preflight._framed_digest(
        ((b"field", b"a"), (b"field", b"bc"))
    ) != preflight._framed_digest(((b"field", b"ab"), (b"field", b"c")))


def test_every_git_subprocess_has_timeout_and_typed_failure(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    seen: list[float | None] = []

    def timeout(*_args: object, **kwargs: object) -> subprocess.CompletedProcess[bytes]:
        seen.append(kwargs.get("timeout"))  # type: ignore[arg-type]
        raise subprocess.TimeoutExpired(["git"], kwargs.get("timeout"))

    monkeypatch.setattr(type(preflight._COMMANDS), "run", timeout)

    with pytest.raises(preflight.PreflightGitError, match="timed out"):
        preflight._git_bytes(tmp_path, "status")
    assert seen == [preflight._GIT_TIMEOUT_SECONDS]


def test_source_identity_changes_for_tracked_and_untracked_content(
    tmp_path: Path,
) -> None:
    def git(*args: str) -> None:
        run_guarded_test_process(
            ["git", *args],
            prefix="MOLT_PYTEST_RUNTIME_WASM_PREFLIGHT_GIT",
            cwd=tmp_path,
            check=True,
        )

    git("init", "-q")
    source = tmp_path / "source.txt"
    source.write_text("first\n", encoding="utf-8")
    git("add", "source.txt")
    git(
        "-c",
        "user.email=test@example.com",
        "-c",
        "user.name=Test User",
        "commit",
        "-q",
        "-m",
        "initial",
    )
    clean = preflight._source_identity(tmp_path)

    source.write_text("second\n", encoding="utf-8")
    tracked = preflight._source_identity(tmp_path)
    untracked_path = tmp_path / "untracked.txt"
    untracked_path.write_text("third\n", encoding="utf-8")
    untracked = preflight._source_identity(tmp_path)

    assert clean["working_tree_digest"] != tracked["working_tree_digest"]
    assert tracked["working_tree_digest"] != untracked["working_tree_digest"]


def test_context_derives_all_roots_from_existing_custody_authorities(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    project = tmp_path / "project"
    custody_root = tmp_path / "custody"
    target = custody_root / "target"
    cache = custody_root / "cache"
    runtime = custody_root / "wasm"
    for path in (project, custody_root, target, cache, runtime):
        path.mkdir(parents=True)
    db = custody_root / "proof.sqlite3"
    _queue_db(db, rows=(_row("run-current"),))
    custody = CheckoutCustody(
        source_root=project,
        custody_root=custody_root,
        toolchain_root=custody_root / "target-root",
        kind="explicit-scratch",
    )
    env = {
        "MOLT_PROOF_QUEUE": "1",
        "MOLT_PROOF_QUEUE_RUN_ID": "run-current",
        "MOLT_PROOF_QUEUE_DB": str(db),
    }
    monkeypatch.setattr(
        preflight, "checkout_custody", lambda *_args, **_kwargs: custody
    )
    monkeypatch.setattr(
        preflight,
        "development_artifact_env",
        lambda *_args, **_kwargs: {
            **env,
            "CARGO_TARGET_DIR": str(target),
            "MOLT_CACHE": str(cache),
            "MOLT_WASM_RUNTIME_DIR": str(runtime),
        },
    )
    monkeypatch.setattr(preflight, "_marker_directories", lambda *_args, **_kwargs: ())

    context = preflight._resolve_preflight_context(project.resolve(), env)

    assert context.roots == preflight.RuntimeWasmPreflightRoots(
        project=project.resolve(),
        custody=custody_root.resolve(),
        target=target.resolve(),
        cache=cache.resolve(),
        runtime=runtime.resolve(),
        proof_queue_db=db.resolve(),
        marker_dirs=(),
    )


def test_claim_binding_requires_live_guard_ancestry(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    claim = preflight.RuntimeWasmBuildClaim(
        run_id="run",
        resource_family="wasm",
        contention_key="wasm:final",
        resource_mutex_key="compiler-build-resource",
        status="running",
        guard_pid=123,
    )
    from tools import memory_guard

    monkeypatch.setattr(memory_guard, "_ancestor_pids", lambda _samples, _pid: {123})
    assert preflight._claim_binds_current_process(claim, {123: object()})
    assert not preflight._claim_binds_current_process(claim, {})
    monkeypatch.setattr(memory_guard, "_ancestor_pids", lambda _samples, _pid: set())
    assert not preflight._claim_binds_current_process(claim, {123: object()})


def test_launch_custody_requires_exact_guard_pid_equality(tmp_path: Path) -> None:
    context = _context(tmp_path)
    with sqlite3.connect(context.roots.proof_queue_db) as conn:
        conn.execute(
            "UPDATE proof_runs SET guard_pid = ? WHERE run_id = ?",
            (context.claim.guard_pid + 1, context.claim.run_id),
        )

    with pytest.raises(preflight.RuntimeWasmPreflightError, match="claim changed"):
        preflight._revalidate_launch_custody(context)


def test_canonical_host_path_rejects_poison_and_filesystem_aliases(
    tmp_path: Path,
) -> None:
    with pytest.raises(PathCustodyError, match="forbidden D"):
        canonical_host_path(
            r"D:\\Molt\\target",
            CustodyPathRole.DURABLE_AUTHORITY,
            authority="test target",
        )

    real = tmp_path / "real"
    alias = tmp_path / "alias"
    real.mkdir()
    try:
        alias.symlink_to(real, target_is_directory=True)
    except OSError:
        pytest.skip("host cannot create directory symlinks")
    with pytest.raises(PathCustodyError, match="canonical filesystem spelling"):
        canonical_host_path(
            alias,
            CustodyPathRole.EXPLICIT_SCRATCH,
            authority="test alias",
            require_exists=True,
        )


def test_cli_refuses_source_only_validation_and_launches_by_exec(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    with pytest.raises(SystemExit):
        preflight.main(["--project-root", str(tmp_path)])

    context = _context(tmp_path)
    monkeypatch.setattr(preflight, "_resolve_preflight_context", lambda *_args: context)
    monkeypatch.setattr(
        preflight,
        "build_preflight",
        lambda **_kwargs: {"schema": preflight.SCHEMA, "status": "ready", "errors": []},
    )
    receipts: list[Path] = []
    monkeypatch.setattr(
        preflight,
        "_atomic_write_text",
        lambda path, _text: receipts.append(path),
    )
    custody_revalidations: list[str] = []
    monkeypatch.setattr(
        preflight,
        "_revalidate_launch_custody",
        lambda current: custody_revalidations.append(current.claim.run_id),
    )
    launched: list[list[str]] = []

    def fake_exec(command: object, _env: object) -> None:
        launched.append(list(command))  # type: ignore[arg-type]
        raise SystemExit(0)

    monkeypatch.setattr(preflight, "_exec_build", fake_exec)

    with pytest.raises(SystemExit, match="0"):
        preflight.main(["--project-root", str(tmp_path), "--launch"])

    assert receipts == [
        context.roots.custody / "logs/runtime_wasm_final_preflight" / "run-current.json"
    ]
    assert "internal-runtime-wasm-build" in launched[0]
    assert "both" in launched[0]
    assert custody_revalidations == ["run-current"]
