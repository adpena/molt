"""Live custody must separate the apparatus's own writes from input mutations.

`git status` (run by custody's source snapshot and by tree validators) refreshes
the index through `.git/index.lock`, and the memory guard writes its state
files. Neither carries information about the proof's inputs, so a proof must
not fail closed as "transient-input-mutation" on them: the git refresh is
classified under its own receipt field and the guard state is routed to the
admitted state root outside the watched tree.
"""

from __future__ import annotations

import sys
import copy
import mmap
import os
from pathlib import Path

import pytest

ROOT = Path(__file__).resolve().parents[2]
if str(ROOT) not in sys.path:
    sys.path.insert(0, str(ROOT))

from molt import dx  # noqa: E402
from molt.custody_layout import out_of_tree_scratch_root  # noqa: E402
from molt.memory_guard_paths import (  # noqa: E402
    harness_guard_artifact_dir,
    pytest_guard_summary_dir,
)
from tools.proof_queue_pkg.execution_custody import (  # noqa: E402
    APPARATUS_GIT_INDEX_REFRESH,
    APPARATUS_UV_ENVIRONMENT_LOCK,
    LiveCustodyMonitor,
    WatchSpec,
    captured_uv_environment_locks,
    classify_apparatus_event,
    validate_apparatus_events,
    validated_uv_environment_lock_capture,
)


def _captured_lock_inputs(root: Path, *, content: bytes = b""):
    """Independent projection inputs and real bytes, never an executable receipt.

    Full toolchain validation belongs to the caller. This fixture supplies the
    exact selected fields consumed by the finite projection; kernel tests below
    use real files/events, and installed qualification supplies real captures.
    """
    import hashlib

    root.mkdir()
    root = root.resolve(strict=True)
    lock = root / ".lock"
    lock.write_bytes(content)
    python = {
        "identity_sha256": "1" * 64,
        "location": {"prefix": str(root)},
        "environment": {
            "tree": {
                "entries": [
                    {
                        "path": ".lock",
                        "kind": "file",
                        "node": "lock-node",
                        "access": {
                            "readable": True,
                            "writable": True,
                            "executable": False,
                        },
                    }
                ],
                "file_nodes": [
                    {
                        "id": "lock-node",
                        "size": len(content),
                        "sha256": hashlib.sha256(content).hexdigest(),
                    }
                ],
            }
        },
        "file_custody": [
            {
                "path": str(lock),
                "size": len(content),
                "sha256": hashlib.sha256(content).hexdigest(),
            }
        ],
    }
    return lock, [{"uv": {"identity_sha256": "2" * 64}}, {"python": python}]


def _lock_captures(inputs):
    return [captured_uv_environment_locks(capture) for capture in inputs]


def _armed_lock_monitor(root: Path):
    spec = WatchSpec(root)
    monitor = LiveCustodyMonitor([spec])
    monitor._transition("CREATED", "ARMED")
    return spec, monitor


def test_uv_lock_joins_separate_capture_owners_without_retaining_inventories(tmp_path):
    lock, inputs = _captured_lock_inputs(tmp_path / "venv")
    captures = _lock_captures(inputs)
    inputs[1]["python"]["environment"]["tree"]["entries"].clear()
    spec, monitor = _armed_lock_monitor(lock.parent)
    monitor.admit_uv_environment_locks(captures)
    monitor._record_event(spec, "inotify:0x8", lock)
    monitor.drain()
    receipt = monitor.receipt()
    assert receipt["stable"] is True
    assert receipt["events"] == []
    assert receipt["apparatus_events"] == [
        {
            "action": "inotify:0x8",
            "path": str(lock),
            "apparatus": "uv-environment-lock-close",
            "python_identity_sha256": "1" * 64,
            "uv_identity_sha256": "2" * 64,
        }
    ]
    validate_apparatus_events(receipt["apparatus_events"], captures)
    with pytest.raises(ValueError, match="armed capture boundary"):
        monitor.admit_uv_environment_locks(captures)


@pytest.mark.parametrize(
    "case", ["no-uv", "no-lock", "nonempty", "symlink-node", "executable"]
)
def test_uv_lock_projection_never_grants_by_filename_alone(tmp_path, case):
    lock, inputs = _captured_lock_inputs(
        tmp_path / "venv", content=b"x" if case == "nonempty" else b""
    )
    if case == "no-uv":
        inputs.pop(0)
    else:
        tree = inputs[1]["python"]["environment"]["tree"]
        if case == "no-lock":
            tree["entries"].clear()
        elif case == "symlink-node":
            tree["entries"][0]["kind"] = "symlink"
        elif case == "executable":
            tree["entries"][0]["access"]["executable"] = True
    spec, monitor = _armed_lock_monitor(lock.parent)
    monitor.admit_uv_environment_locks(_lock_captures(inputs))
    monitor._record_event(spec, "inotify:0x8", lock)
    monitor.drain()
    assert monitor.receipt()["events"] == [{"path": str(lock), "action": "inotify:0x8"}]
    assert monitor.receipt()["stable"] is False


def test_uv_lock_requires_the_same_empty_file_in_endpoint_custody(tmp_path):
    _lock, inputs = _captured_lock_inputs(tmp_path / "venv")
    inputs[1]["python"]["file_custody"] = []
    with pytest.raises(ValueError, match="captured file custody"):
        _lock_captures(inputs)


@pytest.mark.parametrize(
    "case",
    [
        "outside-watch",
        "changed-size",
        "hardlink",
        "symlink",
        "ambiguous-uv",
        "conflicting-python",
    ],
)
def test_uv_lock_admission_refuses_changed_or_ambiguous_ownership(tmp_path, case):
    lock, inputs = _captured_lock_inputs(tmp_path / "venv")
    if case == "ambiguous-uv":
        inputs.append({"uv": {"identity_sha256": "3" * 64}})
    if case == "conflicting-python":
        second = copy.deepcopy(inputs[1])
        second["python"]["identity_sha256"] = "4" * 64
        inputs.append(second)
    captures = _lock_captures(inputs)
    if case == "changed-size":
        lock.write_bytes(b"x")
    elif case == "hardlink":
        os.link(lock, lock.with_name("other"))
    elif case == "symlink":
        original = lock.with_name("original")
        lock.rename(original)
        lock.symlink_to(original)
    root = tmp_path / "unrelated" if case == "outside-watch" else lock.parent
    root.mkdir(exist_ok=True)
    _spec, monitor = _armed_lock_monitor(root)
    with pytest.raises(ValueError):
        monitor.admit_uv_environment_locks(captures)
    monitor.drain()


@pytest.mark.parametrize(
    "action",
    [
        "inotify:0xa",
        "inotify:0xc",
        "inotify:0x100",
        "inotify:0x200",
        "inotify:0x40",
        "inotify:0x80",
        "modified",
        "fsevents:0x1000",
    ],
)
def test_uv_lock_admission_keeps_all_nonpure_close_events(tmp_path, action):
    lock, inputs = _captured_lock_inputs(tmp_path / "venv")
    spec, monitor = _armed_lock_monitor(lock.parent)
    monitor.admit_uv_environment_locks(_lock_captures(inputs))
    monitor._record_event(spec, action, lock)
    monitor.drain()
    assert monitor.receipt()["events"] == [{"action": action, "path": str(lock)}]
    assert monitor.receipt()["stable"] is False


def test_uv_lock_admission_does_not_erase_earlier_or_neighbor_events(tmp_path):
    lock, inputs = _captured_lock_inputs(tmp_path / "venv")
    neighbor = lock.with_name("ordinary.py")
    neighbor.write_bytes(b"")
    spec, monitor = _armed_lock_monitor(lock.parent)
    monitor._record_event(spec, "inotify:0x8", lock)
    monitor.admit_uv_environment_locks(_lock_captures(inputs))
    monitor._record_event(spec, "inotify:0x8", neighbor)
    monitor.drain()
    assert monitor.receipt()["events"] == [
        {"action": "inotify:0x8", "path": str(lock)},
        {"action": "inotify:0x8", "path": str(neighbor)},
    ]


@pytest.mark.parametrize("case", ["replacement", "metadata"])
def test_uv_lock_drain_revalidates_stable_file_even_without_injected_events(
    tmp_path, case
):
    lock, inputs = _captured_lock_inputs(tmp_path / "venv")
    spec, monitor = _armed_lock_monitor(lock.parent)
    monitor.admit_uv_environment_locks(_lock_captures(inputs))
    monitor._record_event(spec, "inotify:0x8", lock)
    if case == "replacement":
        replacement = tmp_path / "replacement"
        replacement.write_bytes(b"")
        replacement.replace(lock)
    else:
        os.utime(lock, ns=(1_000_000_000, 1_000_000_000))
    monitor.drain()
    assert monitor.receipt()["stable"] is False
    assert monitor.receipt()["errors"]


@pytest.mark.parametrize(
    "field",
    [
        "apparatus",
        "removed-class",
        "path",
        "python_identity_sha256",
        "uv_identity_sha256",
        "action",
    ],
)
def test_uv_lock_receiver_rejects_rebound_or_unclassified_events(tmp_path, field):
    lock, inputs = _captured_lock_inputs(tmp_path / "venv")
    captures = _lock_captures(inputs)
    event = {
        "action": "inotify:0x8",
        "path": str(lock),
        "apparatus": APPARATUS_UV_ENVIRONMENT_LOCK,
        "python_identity_sha256": "1" * 64,
        "uv_identity_sha256": "2" * 64,
    }
    if field == "removed-class":
        event.pop("apparatus")
    else:
        event[field] = (
            str(lock.with_name("other")) if field == "path" else "substituted"
        )
    with pytest.raises(ValueError):
        validate_apparatus_events([event], captures)


def test_apparatus_receiver_preserves_only_real_git_refresh_class(tmp_path):
    git = tmp_path / ".git"
    git.mkdir()
    event = {
        "action": "inotify:0x8",
        "path": str(git / "index.lock"),
        "apparatus": APPARATUS_GIT_INDEX_REFRESH,
    }
    validate_apparatus_events([event], [])
    git.rmdir()
    # A durable receipt does not require the historical Git directory to exist.
    validate_apparatus_events([event], [])
    event["path"] = str(tmp_path / ".lock")
    with pytest.raises(ValueError, match="index-refresh authority"):
        validate_apparatus_events([event], [])


@pytest.mark.skipif(
    not sys.platform.startswith("linux"), reason="Linux inotify semantics"
)
@pytest.mark.parametrize(
    "case",
    [
        "lock-no-write",
        "source-no-write",
        "source-mmap-restore",
        "nonempty-mmap-restore",
        "write-restore",
        "truncate-restore",
        "chmod-restore",
        "replace",
        "rename-restore",
        "symlink-replace",
    ],
)
def test_linux_uv_lock_custody_preserves_real_mutation_discriminators(tmp_path, case):
    lock, inputs = _captured_lock_inputs(
        tmp_path / "venv", content=b"abcd" if case == "nonempty-mmap-restore" else b""
    )
    source = lock.with_name("ordinary.py")
    source.write_bytes(b"abcd")
    target = source if case.startswith("source-") else lock
    captures = _lock_captures(inputs)
    monitor = LiveCustodyMonitor([WatchSpec(lock.parent)])
    with monitor:
        monitor.admit_uv_environment_locks(captures)
        if case in {"lock-no-write", "source-no-write"}:
            descriptor = os.open(target, os.O_RDWR)
            os.close(descriptor)
        elif case in {"nonempty-mmap-restore", "source-mmap-restore"}:
            with target.open("r+b") as stream, mmap.mmap(stream.fileno(), 0) as mapping:
                mapping[:] = b"wxyz"
                mapping.flush()
                mapping[:] = b"abcd"
                mapping.flush()
        elif case == "write-restore":
            lock.write_bytes(b"changed")
            lock.write_bytes(b"")
        elif case == "truncate-restore":
            with lock.open("r+b") as stream:
                stream.truncate(8)
                stream.truncate(0)
        elif case == "chmod-restore":
            mode = lock.stat().st_mode & 0o777
            lock.chmod(mode ^ 0o100)
            lock.chmod(mode)
        elif case == "replace":
            replacement = tmp_path / "replacement"
            replacement.write_bytes(b"")
            replacement.replace(lock)
        elif case == "rename-restore":
            moved = lock.with_name("moved")
            lock.rename(moved)
            moved.rename(lock)
        elif case == "symlink-replace":
            lock.unlink()
            lock.symlink_to(source)
    receipt = monitor.receipt()
    if case == "lock-no-write":
        assert receipt["stable"] is True
        assert receipt["events"] == []
        assert receipt["apparatus_events"]
        validate_apparatus_events(receipt["apparatus_events"], captures)
    else:
        assert receipt["stable"] is False
        assert receipt["events"]
    if case in {"nonempty-mmap-restore", "source-mmap-restore"}:
        assert target.read_bytes() == b"abcd"
        assert {event["action"] for event in receipt["events"]} == {"inotify:0x8"}


def test_uv_lock_receiver_requires_semantic_python_owner_digest(tmp_path):
    _lock, inputs = _captured_lock_inputs(tmp_path / "venv")
    python = inputs[1]["python"]
    # Supply the exact full-identity outer shape, but retain a stale semantic
    # identity after changing its lock node. Transport rehashing cannot repair it.
    python.update(
        {
            "schema": "molt.proof-python-toolchain.v3",
            "identity_kind": "executable",
            "source_root": str(tmp_path),
            "node_custody": [],
            "inventory_profile": {},
            "process_images": [],
        }
    )
    python["environment"]["tree"]["file_nodes"][0]["size"] = 1
    with pytest.raises(ValueError, match="python toolchain identity digest is invalid"):
        validated_uv_environment_lock_capture({"python": python})


def test_uv_lock_receiver_requires_semantic_uv_owner_digest(tmp_path):
    from molt.exact_json import canonical_json_sha256

    executable = tmp_path / "uv"
    executable.write_bytes(b"owned uv identity fixture, never executed")
    from tools.proof_queue_pkg.process_image_capture import capture_image

    image = capture_image("uv-launcher", executable, preserve_path=True)
    uv = {
        "version": "uv 0.12.23 (fixture)",
        "path": str(executable),
        "content_path": str(executable),
        "launcher_sha256": image["sha256"],
        "executable_sha256": image["sha256"],
        "process_images": [image],
    }
    uv["identity_sha256"] = canonical_json_sha256(uv)
    assert (
        validated_uv_environment_lock_capture({"uv": uv}).uv_identity_sha256
        == uv["identity_sha256"]
    )
    uv["identity_sha256"] = "0" * 64
    with pytest.raises(
        ValueError, match="uv operational owner identity digest is invalid"
    ):
        validated_uv_environment_lock_capture({"uv": uv})


def test_unrelated_capture_has_no_uv_lock_policy_work(monkeypatch):
    from tools import proof_plan

    def unexpected_policy_read(*args, **kwargs):
        pytest.fail("a capture without Python or uv must not load their policy")

    monkeypatch.setattr(proof_plan.ProofPlan, "load", unexpected_policy_read)
    capture = validated_uv_environment_lock_capture({"node": {}})
    assert capture.uv_identity_sha256 is None
    assert capture.environments == ()


@pytest.mark.parametrize(
    ("relative", "expected"),
    [
        (".git", APPARATUS_GIT_INDEX_REFRESH),
        (".git/index.lock", APPARATUS_GIT_INDEX_REFRESH),
        (".git/index", None),
        (".git/HEAD", None),
        (".git/refs/heads/main", None),
        (".git/objects/ab/cdef", None),
        ("src/molt/__init__.py", None),
        ("index.lock", None),
        ("bench/friends/repos/numpy/.git", APPARATUS_GIT_INDEX_REFRESH),
        ("bench/friends/repos/numpy/.git/index.lock", APPARATUS_GIT_INDEX_REFRESH),
        ("bench/friends/repos/numpy/.git/worktrees/wt", APPARATUS_GIT_INDEX_REFRESH),
        (
            "bench/friends/repos/numpy/.git/worktrees/wt/index.lock",
            APPARATUS_GIT_INDEX_REFRESH,
        ),
        ("bench/friends/repos/numpy/.git/worktrees/wt/HEAD", None),
        ("bench/friends/repos/numpy/.git/worktrees", None),
        ("bench/friends/repos/numpy/.git/index", None),
    ],
)
def test_only_the_index_refresh_is_apparatus(
    tmp_path: Path, relative: str, expected: str | None
) -> None:
    (tmp_path / ".git").mkdir()
    (tmp_path / "bench/friends/repos/numpy/.git/worktrees/wt").mkdir(parents=True)
    assert (
        classify_apparatus_event(tmp_path, tmp_path / relative, "modified") == expected
    )


def test_paths_outside_the_root_are_never_apparatus(tmp_path: Path) -> None:
    other = tmp_path.parent / (tmp_path.name + "-other")
    assert (
        classify_apparatus_event(tmp_path, other / ".git" / "index.lock", "modified")
        is None
    )


def test_monitor_records_apparatus_events_apart_from_input_mutations(
    tmp_path: Path,
) -> None:
    (tmp_path / ".git").mkdir()
    spec = WatchSpec(root=tmp_path)
    monitor = LiveCustodyMonitor([spec])
    monitor._record_event(spec, "modified", tmp_path / ".git")
    monitor._record_event(spec, "created", tmp_path / ".git" / "index.lock")
    monitor._record_event(spec, "created", tmp_path / ".git" / "index.lock")
    monitor._record_event(spec, "modified", tmp_path / ".git" / "index")
    monitor._record_event(spec, "modified", tmp_path / "src" / "a.py")

    receipt = monitor.receipt()
    assert receipt["apparatus_events"] == [
        {
            "action": "modified",
            "path": str(tmp_path / ".git"),
            "apparatus": APPARATUS_GIT_INDEX_REFRESH,
        },
        {
            "action": "created",
            "path": str(tmp_path / ".git" / "index.lock"),
            "apparatus": APPARATUS_GIT_INDEX_REFRESH,
        },
    ]
    assert receipt["events"] == [
        {"action": "modified", "path": str(tmp_path / ".git" / "index")},
        {"action": "modified", "path": str(tmp_path / "src" / "a.py")},
    ]


def test_apparatus_events_alone_leave_custody_stable(tmp_path: Path) -> None:
    (tmp_path / ".git").mkdir()
    spec = WatchSpec(root=tmp_path)
    monitor = LiveCustodyMonitor([spec])
    monitor._record_event(spec, "modified", tmp_path / ".git" / "index.lock")
    monitor._transition("CREATED", "DRAINED")

    receipt = monitor.receipt()
    assert receipt["events"] == []
    assert receipt["errors"] == []
    assert receipt["stable"] is True
    assert len(receipt["apparatus_events"]) == 1


def test_apparatus_events_are_part_of_the_receipt_identity(tmp_path: Path) -> None:
    (tmp_path / ".git").mkdir()
    spec = WatchSpec(root=tmp_path)
    quiet = LiveCustodyMonitor([spec])
    quiet._transition("CREATED", "DRAINED")
    refreshed = LiveCustodyMonitor([spec])
    refreshed._record_event(spec, "modified", tmp_path / ".git" / "index.lock")
    refreshed._transition("CREATED", "DRAINED")

    assert quiet.receipt()["identity_sha256"] != refreshed.receipt()["identity_sha256"]


@pytest.mark.parametrize("state_root", [None, "   "])
@pytest.mark.parametrize("checkout", ["standalone", "molt-src", "worktrees/worker"])
def test_guard_artifacts_default_to_shared_out_of_tree_custody(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
    state_root: str | None,
    checkout: str,
) -> None:
    # The synthetic checkouts are families or a plain clone, not host scratch.
    monkeypatch.setattr(
        dx, "_host_scratch_roots", lambda: ((tmp_path / "ambient").resolve(),)
    )
    repo = tmp_path / checkout
    environ = {} if state_root is None else {"MOLT_MEMORY_GUARD_STATE_ROOT": state_root}
    pytest_root = pytest_guard_summary_dir(repo, environ=environ)
    harness_root = harness_guard_artifact_dir(repo, environ=environ)
    # A family member shares the family root's scratch; a plain clone uses
    # its out-of-tree scratch.
    expected = (
        out_of_tree_scratch_root(repo)
        if checkout == "standalone"
        else tmp_path.resolve() / "tmp"
    )
    assert pytest_root == expected / "pytest-memory-guard"
    assert harness_root == expected / "harness_memory_guard"
    # Independent regression oracle: recording a failed proof must never mutate
    # its watched source, even for an unconfigured checkout or blank selector.
    assert not pytest_root.is_relative_to(repo.resolve())
    assert not harness_root.is_relative_to(repo.resolve())


def test_guard_summary_dir_follows_the_admitted_state_root(tmp_path: Path) -> None:
    state_root = tmp_path / "state"
    environ = {"MOLT_MEMORY_GUARD_STATE_ROOT": str(state_root)}
    assert pytest_guard_summary_dir(tmp_path / "repo", environ=environ) == (
        state_root.resolve().parent / "pytest-memory-guard"
    )


def test_guard_summary_dir_resolves_a_relative_state_root_against_the_repo(
    tmp_path: Path,
) -> None:
    repo = tmp_path / "repo"
    environ = {"MOLT_MEMORY_GUARD_STATE_ROOT": "custody/state"}
    assert pytest_guard_summary_dir(repo, environ=environ) == (
        (repo / "custody").resolve() / "pytest-memory-guard"
    )


def test_harness_command_log_follows_the_admitted_state_root(tmp_path: Path) -> None:
    repo = tmp_path / "repo"
    state_root = tmp_path / "state"
    assert harness_guard_artifact_dir(
        repo, environ={"MOLT_MEMORY_GUARD_STATE_ROOT": str(state_root)}
    ) == (state_root.resolve().parent / "harness_memory_guard")


@pytest.mark.parametrize(
    "action",
    [
        "created",
        "deleted",
        "renamed-from",
        "renamed-to",
        "unknown",
        "inotify:0x40000200",
        "fsevents:0x20200",
        "inotify:bad-mask",
    ],
)
def test_destructive_or_unknown_git_directory_events_are_input_mutations(
    tmp_path: Path, action: str
) -> None:
    git = tmp_path / ".git"
    git.mkdir()
    assert classify_apparatus_event(tmp_path, git, action) is None


@pytest.mark.parametrize(
    "action", ["modified", "inotify:0x40000004", "fsevents:0x20400"]
)
def test_real_directory_metadata_refresh_is_apparatus(
    tmp_path: Path, action: str
) -> None:
    git = tmp_path / ".git"
    git.mkdir()
    assert (
        classify_apparatus_event(tmp_path, git, action) == APPARATUS_GIT_INDEX_REFRESH
    )


def test_linked_worktree_git_file_is_an_input(tmp_path: Path) -> None:
    git = tmp_path / ".git"
    git.write_text("gitdir: elsewhere", encoding="utf-8")
    assert classify_apparatus_event(tmp_path, git, "modified") is None
    git.unlink()
    assert classify_apparatus_event(tmp_path, git, "deleted") is None


def test_symlinked_git_directory_is_not_excused(tmp_path: Path) -> None:
    actual = tmp_path / "actual"
    actual.mkdir()
    git = tmp_path / ".git"
    try:
        git.symlink_to(actual, target_is_directory=True)
    except OSError:
        pytest.skip("symlinks unavailable")
    assert classify_apparatus_event(tmp_path, git, "modified") is None
    assert classify_apparatus_event(tmp_path, git / "index.lock", "created") is None
