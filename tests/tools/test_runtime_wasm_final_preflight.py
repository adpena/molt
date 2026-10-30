from __future__ import annotations

from contextlib import closing
from dataclasses import replace
import json
import os
import sqlite3
import subprocess
from pathlib import Path
from types import SimpleNamespace
import weakref

import pytest

from molt.dx import CheckoutCustody
from molt.process_environment import ProcessEnvironment
from molt.path_custody import PathCustodyError, canonical_host_path
from tests.process_guard_common import run_guarded_test_process
from tools.memory_guard_core import active_custody as custody
from tools import runtime_wasm_final_preflight as preflight
from tools.proof_queue_pkg import state as proof_queue_state


@pytest.mark.parametrize("explicit_state", [False, True])
@pytest.mark.parametrize(
    ("build_profile", "explicit_profile", "requested"),
    [
        ("dev", None, "dev-fast"),
        ("release", None, "release"),
        ("release", "release-output", "release-output"),
    ],
)
def test_planned_pair_uses_production_control_root(
    tmp_path, monkeypatch, explicit_state, build_profile, explicit_profile, requested
) -> None:
    from molt.build_state_layout import build_state_root

    project = tmp_path / "project"
    target = tmp_path / "disposable" / "target"
    custody = tmp_path / "custody"
    ambient_roots = {
        "MOLT_EXT_ROOT": str(tmp_path / "ambient-wrong-root"),
        "MOLT_BUILD_STATE_DIR": str(tmp_path / "ambient-wrong-state"),
    }
    for name, value in ambient_roots.items():
        monkeypatch.setenv(name, value)
    build_env = {
        "MOLT_EXT_ROOT": str(custody),
        "CARGO_TARGET_DIR": str(target),
        "MOLT_CACHE": str(custody / "cache"),
        "MOLT_WASM_RUNTIME_DIR": str(custody / "wasm"),
    }
    monkeypatch.delenv("MOLT_DEV_CARGO_PROFILE", raising=False)
    monkeypatch.delenv("MOLT_RELEASE_CARGO_PROFILE", raising=False)
    if explicit_profile is not None:
        build_env["MOLT_RELEASE_CARGO_PROFILE"] = explicit_profile
    if explicit_state:
        build_env["MOLT_BUILD_STATE_DIR"] = str(custody / "operator-state")
    manifest = SimpleNamespace(digest="toolchain", write=lambda _path: None)
    identity = SimpleNamespace(
        family_digest="f" * 64, digest="identity", toolchain_manifest=manifest
    )
    requests = []

    def compute_spec(*_args, build_profile: str, **kwargs):
        assert build_profile in {"dev", "release"}
        requests.append((build_profile, kwargs["cargo_profile"]))
        from molt.cli.runtime_wasm_build_policy import _resolve_wasm_cargo_profile

        return SimpleNamespace(
            target_root=target,
            cargo_profile=_resolve_wasm_cargo_profile(kwargs["cargo_profile"]),
        )

    monkeypatch.setattr(preflight, "_compute_runtime_wasm_build_spec", compute_spec)
    monkeypatch.setattr(
        preflight, "_resolve_runtime_wasm_cargo_specs", lambda _p, a, b, **k: (a, b)
    )
    monkeypatch.setattr(
        preflight,
        "_resolved_runtime_wasm_family_identities",
        lambda *a: (identity, identity),
    )
    monkeypatch.setattr(
        preflight,
        "_runtime_wasm_toolchain_manifest_path",
        lambda _s: custody / "sdk.json",
    )
    pair = preflight._planned_pair(
        project_root=project,
        target_root=target,
        cache_root=custody / "cache",
        runtime_dir=custody / "wasm",
        build_profile=build_profile,
        stdlib_profile="full",
        build_env=build_env,
    )
    state = build_state_root(
        project_root=project,
        cargo_target=target,
        environment=build_env,
    )
    assert pair["expected_identity"] == str(
        state / "runtime_wasm_generations" / f"{'f' * 64}.expected.json"
    )
    assert state.is_relative_to(custody)
    assert requests == [(build_profile, requested), (build_profile, requested)]
    assert pair["cargo_profile"] == (
        "wasm-release" if requested == "release" else requested
    )
    # The planned build never rewrites the caller's ambient roots.
    assert {name: os.environ.get(name) for name in ambient_roots} == ambient_roots
    assert ("MOLT_BUILD_STATE_DIR" in pair["required_env"]) is explicit_state


def _queue_db(path: Path, *, rows: tuple[tuple[object, ...], ...]) -> None:
    with closing(proof_queue_state._connect(path)) as conn:
        conn.executemany(
            """
            INSERT INTO proof_runs (
                run_id, resource_family, contention_key, resource_mutex_key,
                status, guard_pid, started_at, command_json,
                logical_id, reason, cwd, scopes_json, log_path, summary_json
            ) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
            """,
            (
                (
                    *row,
                    str(row[0]),
                    "independent launch-claim fixture",
                    str(path.parent),
                    "[]",
                    str(path.parent / f"{row[0]}.log"),
                    str(path.parent / f"{row[0]}.summary.json"),
                )
                for row in rows
            ),
        )
        conn.commit()


def _row(
    run_id: str,
    *,
    status: str = "running",
    guard_pid: int | None = None,
    contention_key: str = "wasm:final",
    resource_mutex_key: str | None = "compiler-build-resource",
) -> tuple[object, ...]:
    return (
        run_id,
        "wasm",
        contention_key,
        resource_mutex_key,
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
        build_env=ProcessEnvironment(
            {
                "CARGO_TARGET_DIR": str(target),
                "MOLT_CACHE": str(cache),
                "MOLT_WASM_RUNTIME_DIR": str(runtime),
            }
        ),
    )


def _build_guard_marker(marker_dir: Path, pid: int) -> Path:
    marker_dir.mkdir(parents=True, exist_ok=True)
    token = f"{pid:032x}"
    marker = marker_dir / f"guard-{pid}-{token}.json"
    marker.write_text(
        json.dumps(
            {
                "pid": pid,
                "schema_version": 2,
                "token": token,
                "guard_process": {"pid": pid, "started_at_ns": 100},
                "child_process": {"pid": pid + 1, "started_at_ns": 200},
                "child_launch_state": "recorded",
                "status": "child_running",
                "command": ["python", "-m", "molt.cli", "internal-runtime-wasm-build"],
                "cwd": str(marker_dir.parent),
            }
        ),
        encoding="utf-8",
    )
    return marker


def test_active_build_guards_require_explicit_live_marker_custody(
    tmp_path: Path,
) -> None:
    marker_dir = tmp_path / "active"
    _build_guard_marker(marker_dir, 123)

    assert preflight._active_build_guards((marker_dir,), live_pids=frozenset({123}))
    assert (
        preflight._active_build_guards(
            (marker_dir,), live_pids=frozenset({123}), exclude_pids=frozenset({123})
        )
        == []
    )


def test_active_build_guards_releases_payloads_and_sorts_compact_conflicts(
    tmp_path: Path, monkeypatch
) -> None:
    marker_dir = tmp_path / "active"
    markers = [_build_guard_marker(marker_dir, pid) for pid in (30, 10, 20)]
    payloads = []
    read = custody.read_marker_record

    class Payload(dict):
        pass

    def tracked_read(path):
        assert all(reference() is None for reference in payloads)
        record = read(path)
        assert record.payload is not None
        payload = Payload(record.payload)
        payloads.append(weakref.ref(payload))
        return replace(record, payload=payload)

    monkeypatch.setattr(custody, "read_marker_record", tracked_read)
    conflicts = preflight._active_build_guards(
        (marker_dir,), live_pids=frozenset({10, 20, 30})
    )
    assert [item["marker"] for item in conflicts] == [
        str(path) for path in sorted(markers)
    ]
    assert [item["pid"] for item in conflicts] == [10, 20, 30]
    assert all(item["status"] == "child_running" for item in conflicts)
    assert all(reference() is None for reference in payloads)
    assert all(
        item["command"] == ["python", "-m", "molt.cli", "internal-runtime-wasm-build"]
        and item["cwd"] == str(tmp_path)
        for item in conflicts
    )


def test_active_build_guards_keeps_conflicts_and_continues_after_late_failure(
    tmp_path: Path, monkeypatch
) -> None:
    first_dir, second_dir = tmp_path / "first", tmp_path / "second"
    first = _build_guard_marker(first_dir, 10)
    second = _build_guard_marker(second_dir, 20)
    iterate = preflight.iter_marker_records

    def failing_records(directory):
        yield from iterate(directory)
        if directory == first_dir:
            raise PermissionError("late custody enumeration failure")

    monkeypatch.setattr(preflight, "iter_marker_records", failing_records)
    conflicts = preflight._active_build_guards(
        (second_dir, first_dir), live_pids=frozenset({10, 20})
    )
    assert [item["marker"] for item in conflicts] == [
        str(first_dir),
        str(first),
        str(second),
    ]
    assert conflicts[0] == {
        "marker": str(first_dir),
        "error": "late custody enumeration failure",
    }
    assert [item["pid"] for item in conflicts[1:]] == [10, 20]
    assert first.exists() and second.exists()


def test_proof_queue_claim_is_typed_and_competing_claims_fail_closed(
    tmp_path: Path,
) -> None:
    db = tmp_path / "proof.sqlite3"
    # The real producer enforces unique launched contention/mutex claims.
    # A distinct historical claim with a NULL serialized mutex is still a
    # competing WASM build by the existing resource-family contract.
    _queue_db(
        db,
        rows=(
            _row("run-current"),
            _row("run-other", contention_key="wasm:competing", resource_mutex_key=None),
        ),
    )

    claim = preflight._proof_queue_claim(db, "run-current")
    conflicts = preflight._proof_queue_conflicts(db, exclude_run_id="run-current")

    assert claim.resource_mutex_key == "compiler-build-resource"
    assert [row["run_id"] for row in conflicts] == ["run-other"]
    assert conflicts[0]["resource_mutex_key"] is None


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
        preflight, "_planned_pair", lambda **_kwargs: {"family_digest": "ab" * 32}
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
        preflight, "_planned_pair", lambda **_kwargs: {"family_digest": "ab" * 32}
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
        lambda *_args, **_kwargs: ProcessEnvironment(
            {
                **env,
                "CARGO_TARGET_DIR": str(target),
                "MOLT_CACHE": str(cache),
                "MOLT_WASM_RUNTIME_DIR": str(runtime),
            }
        ),
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


def test_canonical_host_path_rejects_filesystem_aliases(
    tmp_path: Path,
) -> None:
    admitted = tmp_path / "OneDrive" / "target"
    admitted.mkdir(parents=True)
    assert (
        canonical_host_path(
            admitted,
            authority="test target",
            require_exists=True,
        )
        == admitted.resolve()
    )
    with pytest.raises(PathCustodyError, match="cannot contain"):
        canonical_host_path(
            admitted / ".." / "target",
            authority="test traversal",
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
            authority="test alias",
            require_exists=True,
        )


@pytest.mark.parametrize(
    "reserve_args, expected_bytes",
    [([], 8589934592), (["--reserve-gib=0"], 0), (["--reserve-gib=0.0000000005"], 1)],
)
def test_cli_refuses_source_only_validation_and_forwards_build_status(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch, reserve_args, expected_bytes
) -> None:
    with pytest.raises(SystemExit):
        preflight.main(["--project-root", str(tmp_path)])

    context = _context(tmp_path)
    monkeypatch.setattr(preflight, "_resolve_preflight_context", lambda *_args: context)
    budgets = []

    def build(**kwargs):
        budgets.append(kwargs["reserve_bytes"])
        return {"schema": preflight.SCHEMA, "status": "ready", "errors": []}

    monkeypatch.setattr(preflight, "build_preflight", build)
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

    def fake_forward(command: object, *, env: object) -> int:
        assert env is context.build_env
        launched.append(list(command))  # type: ignore[arg-type]
        return 17

    monkeypatch.setattr(preflight, "forward_command", fake_forward)

    assert (
        preflight.main(["--project-root", str(tmp_path), "--launch", *reserve_args])
        == 17
    )
    assert budgets == [expected_bytes]

    assert receipts == [
        context.roots.custody
        / "logs/runtime_wasm_final_preflight/v4"
        / "run-current.json"
    ]
    assert "internal-runtime-wasm-build" in launched[0]
    assert "both" in launched[0]
    assert custody_revalidations == ["run-current"]


@pytest.mark.parametrize(
    "estimated", [0, 8 * 1024**3 - 1, 8 * 1024**3, OSError("capacity unavailable")]
)
def test_disk_facts_use_native_capacity_but_keep_raw_telemetry(
    tmp_path, monkeypatch, estimated
):
    from molt import disk_capacity

    monkeypatch.setattr(disk_capacity, "sys", SimpleNamespace(platform="darwin"))
    monkeypatch.setattr(disk_capacity, "_default_measure_free_bytes", lambda path: 7)

    def native(path):
        if isinstance(estimated, Exception):
            raise estimated
        return estimated

    monkeypatch.setattr(disk_capacity, "_darwin_important_usage_bytes", native)
    facts = preflight._disk_facts([tmp_path], reserve_bytes=8 * 1024**3)
    assert len(facts) == 1
    fact = facts[0]
    assert fact["free_bytes"] == 7
    assert fact["measurement"] == "darwin-important-usage"
    assert fact["available_bytes"] == (
        None if isinstance(estimated, Exception) else estimated
    )
    assert fact["ready"] is (estimated == 8 * 1024**3)
    assert bool(fact["error"]) is isinstance(estimated, Exception)


def test_disk_facts_distinguish_mounts_with_same_root_spelling_and_measure_once(
    tmp_path,
    monkeypatch,
):
    from molt.disk_capacity import DiskCapacityProbe

    class MountedPath(type(Path())):
        def stat(self, *args, **kwargs):
            return SimpleNamespace(st_dev=11 if "first" in self.parts else 22)

    first = MountedPath(tmp_path / "first" / "outputs")
    second = MountedPath(tmp_path / "second" / "outputs")
    assert first.anchor == second.anchor
    volume_labels = {
        str(first): "apple-uuid:" + "a" * 32,
        str(second): "apple-uuid:" + "b" * 32,
    }
    monkeypatch.setattr(
        preflight, "_durable_volume", lambda path: volume_labels[str(path)]
    )
    monkeypatch.setattr(
        preflight, "_nearest_existing_directory", lambda path: MountedPath(path)
    )
    # These are independent filesystem facts, not values derived from the grouping key.
    calls = []

    def probe(path, *, required_bytes):
        calls.append(path)
        raw = 10 if path == first else 2
        return DiskCapacityProbe(path, path, raw, required_bytes)

    monkeypatch.setattr(preflight, "probe_capacity", probe)
    monkeypatch.setattr(
        preflight,
        "shutil",
        SimpleNamespace(
            disk_usage=lambda path: SimpleNamespace(
                total=100,
                used=90 if path == first else 98,
                free=10 if path == first else 2,
            )
        ),
    )
    facts = preflight._disk_facts([first, second, first], reserve_bytes=8)
    assert len(facts) == 2
    assert calls == [first, second]
    assert [fact["volume"] for fact in facts] == [
        volume_labels[str(first)],
        volume_labels[str(second)],
    ]
    assert [fact["ready"] for fact in facts] == [True, False]
    assert [fact["free_bytes"] for fact in facts] == [10, 2]


@pytest.mark.parametrize(
    "configured", ["-1", "nan", "inf", "-inf", "1e999", "8589934592"]
)
def test_wasm_reserve_rejects_invalid_value_before_queue_context(
    tmp_path, monkeypatch, capsys, configured
):
    monkeypatch.setattr(
        preflight,
        "_resolve_preflight_context",
        lambda *args: pytest.fail("invalid reserve resolved queue context"),
    )
    assert (
        preflight.main(
            ["--project-root", str(tmp_path), "--launch", f"--reserve-gib={configured}"]
        )
        == 2
    )
    payload = json.loads(capsys.readouterr().out)
    assert payload["status"] == "blocked"
    assert payload["errors"][0].startswith(
        "RuntimeWasmPreflightError: invalid --reserve-gib"
    )


@pytest.mark.parametrize("reader", ["claim", "conflicts"])
@pytest.mark.parametrize("condition", ["missing", "unsupported"])
def test_queue_launch_readers_refuse_unavailable_schema_without_repair(
    tmp_path: Path, reader: str, condition: str
) -> None:
    db = tmp_path / "proof.sqlite3"
    if condition == "unsupported":
        _queue_db(db, rows=(_row("run-current"),))
        with closing(sqlite3.connect(db)) as writer:
            writer.execute("ALTER TABLE proof_notes DROP COLUMN body")
            writer.commit()
    before = db.read_bytes() if db.exists() else None
    with pytest.raises(
        preflight.RuntimeWasmPreflightError, match="cannot inspect proof queue"
    ):
        if reader == "claim":
            preflight._proof_queue_claim(db, "run-current")
        else:
            preflight._proof_queue_conflicts(db, exclude_run_id="run-current")
    assert (db.read_bytes() if db.exists() else None) == before


def test_queue_launch_readers_observe_live_wal_and_close_before_return(
    tmp_path: Path, monkeypatch
) -> None:
    db = tmp_path / "proof.sqlite3"
    # The real producer enforces unique launched contention/mutex claims.
    # A distinct historical claim with a NULL serialized mutex is still a
    # competing WASM build by the existing resource-family contract.
    _queue_db(
        db,
        rows=(
            _row("run-current"),
            _row("run-other", contention_key="wasm:competing", resource_mutex_key=None),
        ),
    )
    opened = []
    original_connect = proof_queue_state._connect

    def record_connection(db, *, access="write"):
        conn = original_connect(db, access=access)
        if access == "read":
            opened.append(conn)
        return conn

    monkeypatch.setattr(proof_queue_state, "_connect", record_connection)
    with closing(sqlite3.connect(db)) as writer:
        writer.execute("PRAGMA wal_autocheckpoint=0")
        writer.execute(
            "UPDATE proof_runs SET guard_pid=24680 WHERE run_id='run-current'"
        )
        writer.execute(
            "UPDATE proof_runs SET started_at='wal-only-order' WHERE run_id='run-other'"
        )
        writer.commit()
        wal = Path(str(db) + "-wal")
        assert wal.is_file()
        before = (db.read_bytes(), wal.read_bytes(), tuple(writer.iterdump()))
        claim = preflight._proof_queue_claim(db, "run-current")
        conflicts = preflight._proof_queue_conflicts(db, exclude_run_id="run-current")
        assert claim.guard_pid == 24680
        assert len(conflicts) == 1 and conflicts[0]["run_id"] == "run-other"
        assert conflicts[0]["started_at"] == "wal-only-order"
        assert (db.read_bytes(), wal.read_bytes(), tuple(writer.iterdump())) == before
    assert len(opened) == 2
    for conn in opened:
        with pytest.raises(sqlite3.ProgrammingError, match="closed"):
            conn.execute("SELECT 1")
