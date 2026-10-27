"""Published policy, captured scope and durable launch-custody controls."""

import json
import os
from pathlib import Path
import shutil
import subprocess
import sys
import threading
import time
from types import SimpleNamespace

import pytest

from molt.checkout_state import GC_STAMP
from molt.file_locks import _release_file_lock, _try_acquire_file_lock
from tools import checkout_maintenance as maintenance
from tools import molt_dev_controller as controllers
from tools import molt_dev_detached as detached


def _git(root, *args):
    return subprocess.check_output(
        ["git", "-C", str(root), *args], text=True, encoding="utf-8"
    ).strip()


@pytest.fixture
def published_source(tmp_path, monkeypatch):
    repo = tmp_path / "source"
    repo.mkdir()
    _git(repo, "init", "-q")
    _git(repo, "config", "user.email", "fixture@example.invalid")
    _git(repo, "config", "user.name", "Fixture")
    for name, content in {
        "tools/checkout_maintenance.py": "# published collector\n",
        "src/molt/__init__.py": "",
        "src/sitecustomize.py": "",
        "config/policy.toml": "",
        "pyproject.toml": "[project]\nname='fixture'\nversion='0'\n",
        "uv.lock": "version=1\n",
        ".python-version": "3.12\n",
    }.items():
        path = repo / name
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_text(content, encoding="utf-8")
    _git(repo, "add", "--", ".")
    _git(repo, "commit", "-qm", "Published fixture")
    published = _git(repo, "rev-parse", "HEAD")
    _git(repo, "update-ref", "refs/remotes/origin/main", published)
    monkeypatch.setattr(maintenance, "_REPO_ROOT", repo)
    return repo, published


def test_recent_epoch_reads_no_source_or_launch_authority(tmp_path, monkeypatch):
    root = tmp_path / "artifacts"
    root.joinpath(*GC_STAMP).parent.mkdir(parents=True)
    root.joinpath(*GC_STAMP).touch()

    def forbidden(*_args, **_kwargs):
        raise AssertionError("warm completion entered source/launch authority")

    monkeypatch.setattr(maintenance, "git_directories", forbidden)
    monkeypatch.setattr(detached, "launch_detached", forbidden)
    assert maintenance.schedule(root, {}, cwd=tmp_path) is None


@pytest.mark.parametrize("from_controller", [False, True])
def test_dispatch_uses_published_bytes_and_preserves_relative_scope(
    tmp_path, monkeypatch, published_source, from_controller
):
    repo, published = published_source
    if from_controller:
        with controllers.controller_source(repo, repo / ".git", published) as snapshot:
            monkeypatch.setattr(maintenance, "_REPO_ROOT", snapshot.root)
    # Caller edits cannot become reclamation policy through an implicit snapshot.
    (repo / "tools/checkout_maintenance.py").write_text(
        "raise RuntimeError('dirty caller')\n", encoding="utf-8"
    )
    root = tmp_path / "artifacts"
    root.mkdir()
    cwd = tmp_path / "original-cwd"
    cwd.mkdir()
    observed = []

    def launch(state, command, *, cwd, environment, controller, replace=False):
        assert not replace
        record = json.loads(
            (controller.root / controllers.RECORD_NAME).read_text(encoding="utf-8")
        )
        assert record["commit"] == published
        assert (controller.root / "tools/checkout_maintenance.py").read_text(
            encoding="utf-8"
        ) == "# published collector\n"
        # Parent source use must overlap birth/acknowledgement; retention cannot
        # acquire its exclusive lock while this callback represents the child.
        with controllers.controller_source_use(controller) as use:
            exclusive = _try_acquire_file_lock(Path(use.registry_key))
            if exclusive is not None:
                _release_file_lock(exclusive)
            assert exclusive is None
        observed.append((state, command, cwd, dict(environment), controller))
        return 123

    monkeypatch.setattr(detached, "launch_detached", launch)
    state = maintenance.schedule(
        root,
        {"CARGO_TARGET_DIR": "target-one", "MOLT_DIFF_CARGO_TARGET_DIR": "target-two"},
        cwd=cwd,
    )
    assert state == root / "logs/disk_guard/automatic/gc-initial"
    assert not root.joinpath(*GC_STAMP).exists(), "only the collector admits a day"
    _, command, actual_cwd, env, controller = observed[0]
    assert actual_cwd == controller.root
    assert command[0] == sys.executable and command[1:4] == ["-I", "-S", "-B"]
    assert command[-2:] == ["--policy-cwd", str(cwd)]
    assert env["MOLT_EXT_ROOT"] == str(root)
    assert env["CARGO_TARGET_DIR"] == str(cwd / "target-one")
    assert env["MOLT_DIFF_CARGO_TARGET_DIR"] == str(cwd / "target-two")


def test_existing_epoch_keeps_unresolved_evidence_and_stamp(
    tmp_path, monkeypatch, published_source
):
    root = tmp_path / "artifacts"
    state = root / "logs/disk_guard/automatic/gc-initial"
    state.mkdir(parents=True)
    evidence = state / "worker.json"
    evidence.write_text('{"unresolved":true}', encoding="utf-8")
    monkeypatch.setattr(
        detached,
        "launch_detached",
        lambda *_a, **_k: pytest.fail("replaced an unresolved automatic attempt"),
    )
    with pytest.raises(RuntimeError, match="already has custody"):
        maintenance.schedule(root, {}, cwd=tmp_path)
    assert evidence.read_text(encoding="utf-8") == '{"unresolved":true}'
    assert not root.joinpath(*GC_STAMP).exists()


def test_unpublished_worker_contract_defers_without_daily_admission(
    tmp_path, published_source
):
    repo, _published = published_source
    _git(repo, "rm", "--", "tools/checkout_maintenance.py")
    (repo / "tools").mkdir(exist_ok=True)
    (repo / "tools/legacy.py").write_text("# old controller\n", encoding="utf-8")
    _git(repo, "add", "--", "tools/legacy.py")
    _git(repo, "commit", "-qm", "Older published contract")
    _git(repo, "update-ref", "refs/remotes/origin/main", "HEAD")
    root = tmp_path / "artifacts"
    root.mkdir()
    with pytest.raises(RuntimeError, match="no automatic maintenance entrypoint"):
        maintenance.schedule(root, {}, cwd=tmp_path)
    assert not root.joinpath(*GC_STAMP).exists()
    assert not (root / "logs/disk_guard/automatic/gc-initial").exists()


def test_concurrent_daily_admission_cannot_consume_the_next_epoch(
    tmp_path, monkeypatch, published_source
):
    root = tmp_path / "artifacts"
    stamp = root.joinpath(*GC_STAMP)
    stamp.parent.mkdir(parents=True)
    stamp.touch()
    old = time.time() - 2 * 86400
    os.utime(stamp, (old, old))
    generation = stamp.stat().st_mtime_ns
    original_stat = Path.stat
    observations = []

    def interleaved_stat(path, *args, **kwargs):
        observed = original_stat(path, *args, **kwargs)
        if path == stamp and not observations:
            observations.append(observed.st_mtime_ns)
            # A competing worker admits the day after this caller's read.
            os.utime(stamp, None)
        return observed

    launched = []
    monkeypatch.setattr(Path, "stat", interleaved_stat)
    monkeypatch.setattr(
        detached,
        "launch_detached",
        lambda state, *_args, **_kwargs: launched.append(state),
    )
    state = maintenance.schedule(root, {}, cwd=tmp_path)
    assert observations == [generation]
    assert state == root / f"logs/disk_guard/automatic/gc-{generation}"
    assert launched == [state]
    fresh = original_stat(stamp).st_mtime_ns
    assert fresh != generation
    assert not (root / f"logs/disk_guard/automatic/gc-{fresh}").exists()
    assert maintenance.schedule(root, {}, cwd=tmp_path) is None


def test_worker_publishes_result_errors_as_nonzero(tmp_path, monkeypatch, capsys):
    from tools import disk_guard

    result = SimpleNamespace(
        errors=["refused"], to_dict=lambda: {"errors": ["refused"]}
    )
    monkeypatch.setattr(disk_guard, "collect_due", lambda **_kwargs: [result])
    retired = tmp_path / "retired"
    assert (
        maintenance.main(["--root", str(tmp_path), "--policy-cwd", str(retired)]) == 1
    )
    payload = json.loads(capsys.readouterr().out)
    assert payload["results"] == [{"errors": ["refused"]}]
    assert payload["policy_cwd"] == str(retired)


def test_worker_exception_remains_failure(tmp_path, monkeypatch):
    from tools import disk_guard

    def failure(**_kwargs):
        raise RuntimeError("collector failed")

    monkeypatch.setattr(disk_guard, "collect_due", failure)
    with pytest.raises(RuntimeError, match="collector failed"):
        maintenance.main(["--root", str(tmp_path), "--policy-cwd", str(tmp_path)])


def test_real_dispatch_returns_before_collection_and_survives_lane_removal(
    tmp_path, monkeypatch, record_property
):
    """An independent barrier blocks the real collector, not its dispatcher."""
    from molt import checkout_state, dx

    source = Path(__file__).resolve().parents[2]
    main = tmp_path / "main"
    main.mkdir()
    paths = subprocess.check_output(
        [
            "git",
            "-C",
            str(source),
            "ls-files",
            "-z",
            "--cached",
            "--others",
            "--exclude-standard",
            "--",
            *controllers.CONTROLLER_PATHS,
        ]
    )
    for raw in paths.split(b"\0"):
        if not raw:
            continue
        name = os.fsdecode(raw)
        destination = main / name
        destination.parent.mkdir(parents=True, exist_ok=True)
        shutil.copy2(source / name, destination, follow_symlinks=False)
    # Fault injection belongs to this independently committed fixture policy.
    collector = main / "tools/disk_guard.py"
    with collector.open("a", encoding="utf-8") as stream:
        stream.write(
            "\n_real_collect_due = collect_due\n"
            "def collect_due(*, root=None, env=None):\n"
            "    barrier = Path(env['CHECKOUT_MAINTENANCE_BARRIER'])\n"
            "    barrier.with_suffix('.entered').write_text('entered', encoding='utf-8')\n"
            "    while not barrier.exists():\n"
            "        time.sleep(0.01)\n"
            "    return _real_collect_due(root=root, env=env)\n"
        )
    _git(main, "init", "-q")
    _git(main, "config", "user.email", "fixture@example.invalid")
    _git(main, "config", "user.name", "Fixture")
    _git(main, "add", "--", ".")
    _git(main, "commit", "-qm", "Published collector barrier fixture")
    _git(main, "update-ref", "refs/remotes/origin/main", "HEAD")
    lane = tmp_path / "originating-lane"
    _git(main, "worktree", "add", "-qb", "fixture-lane", str(lane))
    monkeypatch.setattr(maintenance, "_REPO_ROOT", lane)

    artifacts = tmp_path / "artifacts"
    artifacts.mkdir()
    projects = tmp_path / "projects"
    owner = projects / "retired"
    owner.mkdir(parents=True)
    retired = dx.cargo_target_dir_for_artifact_root(artifacts, None, source_root=owner)
    retired.mkdir(parents=True)
    (retired / "payload").write_text("owned fixture bytes", encoding="utf-8")
    checkout_state.record_checkout_owner(retired, owner=owner, kind="cargo-target")
    owner.rmdir()
    metadata = checkout_state._state_metadata_root(retired)
    aged = time.time() - 3 * 3600
    for path in {*retired.iterdir(), retired, *metadata.iterdir(), metadata}:
        os.utime(path, (aged, aged))

    barrier = tmp_path / "release-collector"
    environment = dict(os.environ)
    environment["CHECKOUT_MAINTENANCE_BARRIER"] = str(barrier)
    # A poisoned ambient source cannot replace the snapshot's startup imports.
    environment["PYTHONPATH"] = str(lane / "poisoned-imports")
    result = {}
    returned = threading.Event()

    def dispatch():
        started = time.monotonic()
        try:
            result["state"] = maintenance.schedule(artifacts, environment, cwd=lane)
        except BaseException as error:
            result["error"] = error
        finally:
            result["dispatch_elapsed_s"] = time.monotonic() - started
            returned.set()

    thread = threading.Thread(target=dispatch)
    thread.start()
    state = artifacts / "logs/disk_guard/automatic/gc-initial"
    try:
        assert returned.wait(30), "dispatch waited for the blocked collector"
        if "error" in result:
            raise result["error"]
        assert result["state"] == state
        record_property("maintenance_dispatch_elapsed_s", result["dispatch_elapsed_s"])
        deadline = time.monotonic() + 30
        while not barrier.with_suffix(".entered").exists():
            assert time.monotonic() < deadline, "collector never reached its barrier"
            if (state / "rc").exists():
                pytest.fail((state / "run.log").read_text(encoding="utf-8"))
            time.sleep(0.02)
        assert not (state / "rc").exists()
        assert retired.exists(), "reclamation is still blocked independently"
        _git(main, "worktree", "remove", str(lane))
        assert not lane.exists()
    finally:
        barrier.touch()
        thread.join(35)
        assert not thread.is_alive(), "fixture dispatcher did not close"
        if (state / "worker.json").exists():
            deadline = time.monotonic() + 30
            while not (state / "rc").exists():
                assert time.monotonic() < deadline, "fixture worker did not close"
                time.sleep(0.02)
    assert (state / "rc").read_text(encoding="utf-8").strip() == "0", (
        state / "run.log"
    ).read_text(encoding="utf-8")
    assert not retired.exists()
    logs = list((artifacts / "logs/disk_guard").glob("disk_guard-*.json"))
    assert {
        json.loads(path.read_text(encoding="utf-8"))["operation"] for path in logs
    } == {"gc", "pressure"}
    # The next eligible owner can itself be the surviving pinned controller.
    # Reuse the exact published source without copying or provisioning it again.
    descriptor = json.loads((state / "worker.json").read_text(encoding="utf-8"))[
        "controller"
    ]
    snapshot = Path(descriptor["root"])
    monkeypatch.setattr(maintenance, "_REPO_ROOT", snapshot)
    warm_root = tmp_path / "artifacts-cached-source"
    warm_root.mkdir()
    started = time.monotonic()
    warm_state = maintenance.schedule(warm_root, environment, cwd=snapshot)
    record_property("maintenance_cached_dispatch_elapsed_s", time.monotonic() - started)
    assert warm_state is not None
    deadline = time.monotonic() + 30
    while not (warm_state / "rc").exists():
        assert time.monotonic() < deadline, "cached-source fixture worker did not close"
        time.sleep(0.02)
    assert (warm_state / "rc").read_text(encoding="utf-8").strip() == "0", (
        warm_state / "run.log"
    ).read_text(encoding="utf-8")
