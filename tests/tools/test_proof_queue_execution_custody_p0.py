from __future__ import annotations

import json
import hashlib
import os
import runpy
import sys
import time
from pathlib import Path

import pytest

from tests.process_guard_common import run_custody_subject_process

from tools.proof_queue_pkg import execution_custody, supervisor_custody


def test_python_payload_cannot_replace_private_audit_enforcement(
    tmp_path: Path,
) -> None:
    bootstrap = Path(execution_custody.__file__).with_name(
        "python_custody_bootstrap.py"
    )
    marker = tmp_path / "escaped"
    child = f"from pathlib import Path; Path({str(marker)!r}).touch()"
    payload = (
        "import subprocess,sys; "
        "assert '_molt_proof_execution_custody' not in sys.modules; "
        "blocked=False\n"
        "try:\n"
        f" subprocess.run([sys.executable,'-c',{child!r}],check=True)\n"
        "except PermissionError:\n"
        " blocked=True\n"
        "assert blocked"
    )
    policy = {
        "schema": execution_custody.CHILD_POLICY_SCHEMA,
        "descendants": "forbidden",
        "allowed": [],
    }
    server = execution_custody.ChildCustodyEventServer("python", policy)
    environment = dict(os.environ)
    environment[execution_custody.CHILD_POLICY_ENV] = json.dumps(policy)
    environment.update(server.environment())

    with server:
        completed = run_custody_subject_process(
            [sys.executable, bootstrap, "command", "0", payload],
            env=environment,
            check=False,
            capture_output=True,
            text=True,
            timeout=20,
        )

    receipt = server.receipt()
    assert completed.returncode == 0, completed.stderr
    assert not marker.exists()
    assert receipt["broker_complete"] is True
    assert receipt["process_closure_complete"] is False
    assert receipt["scope"] == "runtime-hook-broker"
    assert receipt["violations"]
    assert not execution_custody.child_receipt_is_admitted(receipt)


@pytest.mark.parametrize("runtime", ["python", "node"])
def test_child_admission_distinguishes_handshakes_from_launch_decisions(runtime):
    receipt = {
        "broker_complete": True,
        "errors": [],
        "violations": [],
        "events": [
            {"event": "hook-start", "runtime": runtime, "connection_id": 0},
            {"event": "child-process", "admitted": True},
            {"event": "hook-end", "runtime": runtime, "connection_id": 0},
        ],
    }
    assert execution_custody.child_receipt_is_admitted(receipt)
    for bad_event in (
        {"event": "child-process", "admitted": False},
        {"event": "child-process"},
        {"event": "unknown", "admitted": True},
        {"event": "hook-start"},
        {"event": [], "admitted": True},
        None,
    ):
        assert not execution_custody.child_receipt_is_admitted(
            {**receipt, "events": [*receipt["events"], bad_event]}
        )
    for field, value in (
        ("broker_complete", False),
        ("errors", ["broken transport"]),
        ("violations", [{"event": "child-process", "admitted": False}]),
    ):
        assert not execution_custody.child_receipt_is_admitted(
            {**receipt, field: value}
        )


def test_derived_child_admission_reuses_supervisor_provenance(tmp_path: Path):
    source = tmp_path / "source"
    source.mkdir()
    scratch = tmp_path / "scratch"
    scratch.mkdir()
    provenance = supervisor_custody._derived_root_provenance(
        descendants="declared-toolchains",
        env={supervisor_custody.PROOF_SCRATCH_ROOT_ENV: str(scratch)},
        source_root=source,
        result_path=tmp_path / "receipt.json",
    )
    envelope = {"process_closure": {"descendants": "declared-toolchains"}}
    policy = execution_custody.child_policy(envelope, {}, derived_roots=provenance)
    role = supervisor_custody.SCRATCH_OUTPUT_ROLE
    assert policy["derived_roots"] == [{"role": role, "path": str(scratch.resolve())}]
    with pytest.raises(ValueError, match="run-owned provenance"):
        execution_custody.child_policy(
            envelope, {}, derived_roots=[{**provenance[0], "run_owned": False}]
        )
    with pytest.raises(ValueError, match="run-owned provenance"):
        execution_custody.child_policy(
            {"process_closure": {"descendants": "forbidden"}},
            {},
            derived_roots=provenance,
        )

    image = scratch / "candidate.exe"
    image.write_bytes(b"generated native image")
    outside = tmp_path / "scratch-sibling"
    outside.mkdir()
    escaped_image = outside / "candidate.exe"
    escaped_image.write_bytes(image.read_bytes())
    server = execution_custody.ChildCustodyEventServer(None, policy)
    with server:
        admitted = server._decide_child({"requested": str(image)})
        assert admitted["admitted"] is True
        assert admitted["derived_role"] == role
        assert admitted["resolved"] == str(image.resolve())
        assert admitted["sha256"] == hashlib.sha256(image.read_bytes()).hexdigest()
        denied = server._decide_child({"requested": str(escaped_image)})
        assert denied["admitted"] is False


def test_derived_child_admission_rejects_symlink_escape(tmp_path: Path):
    root = tmp_path / "owned"
    root.mkdir()
    outside = tmp_path / "outside.exe"
    outside.write_bytes(b"outside native image")
    link = root / "escape.exe"
    try:
        link.symlink_to(outside)
    except OSError as exc:
        if os.name == "nt" and exc.winerror == 1314:
            pytest.skip("Windows host lacks symbolic-link creation privilege")
        raise
    policy = execution_custody.child_policy(
        {"process_closure": {"descendants": "declared-toolchains"}},
        {},
        derived_roots=[
            {"role": "scratch-output", "path": str(root), "run_owned": True}
        ],
    )
    with execution_custody.ChildCustodyEventServer(None, policy) as server:
        assert server._decide_child({"requested": str(link)})["admitted"] is False


@pytest.mark.parametrize("changed_field", [None, "path", "sha256", "roles", "class"])
def test_derived_child_binds_actual_executed_image(tmp_path: Path, changed_field):
    image = {
        "class": "derived",
        "path": str(tmp_path / "candidate.exe"),
        "sha256": hashlib.sha256(b"generated native image").hexdigest(),
        "roles": ["scratch-output"],
    }
    receipt = {
        "events": [
            {
                "event": "child-process",
                "admitted": True,
                "resolved": image["path"],
                "sha256": image["sha256"],
                "derived_role": "scratch-output",
            }
        ]
    }
    event_log = tmp_path / "events.jsonl"
    if changed_field is not None:
        image[changed_field] = (
            ["build-output"] if changed_field == "roles" else "changed"
        )
    event_log.write_text(
        json.dumps({"kind": "exec", "image": image}) + "\n", encoding="utf-8"
    )
    if changed_field is None:
        execution_custody.require_derived_child_image_bindings(receipt, event_log)
    else:
        with pytest.raises(ValueError, match="executed image identity"):
            execution_custody.require_derived_child_image_bindings(receipt, event_log)
    event_log.write_text("", encoding="utf-8")
    with pytest.raises(ValueError, match="executed image identity"):
        execution_custody.require_derived_child_image_bindings(receipt, event_log)


def test_source_watch_records_create_execute_delete_transient(
    tmp_path: Path,
) -> None:
    source = tmp_path / "source"
    source.mkdir()
    tracked = source / "tracked.py"
    tracked.write_text("VALUE = 1\n", encoding="utf-8")
    before = tracked.read_bytes()
    specs = execution_custody.watch_specs(
        source_root=source,
        tracked_paths=[tracked],
        identities=[],
        broad_roots=[],
    )
    assert specs == [execution_custody.WatchSpec(source.resolve(), None)]
    monitor = execution_custody.LiveCustodyMonitor(specs)

    with monitor:
        transient = source / "transient.py"
        transient.write_text("EXECUTED = True\n", encoding="utf-8")
        namespace = runpy.run_path(str(transient))
        assert namespace["EXECUTED"] is True
        transient.unlink()
        # Kernel delivery is asynchronous even though enqueue is synchronous.
        time.sleep(0.10)

    assert tracked.read_bytes() == before
    assert not transient.exists()
    receipt = monitor.receipt()
    assert receipt["stable"] is False
    assert any(
        Path(str(event["path"])).name == "transient.py" for event in receipt["events"]
    )


def test_linux_root_watch_is_installed_before_recursive_enumeration(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    root = tmp_path / "root"
    root.mkdir()
    monitor = execution_custody.LiveCustodyMonitor(
        [execution_custody.WatchSpec(root, None)]
    )
    add_calls: list[Path] = []

    class FakeFunction:
        def __init__(self, implementation):
            self.implementation = implementation

        def __call__(self, *args):
            return self.implementation(*args)

    class FakeLibc:
        inotify_init1 = FakeFunction(lambda _flags: 17)

        @staticmethod
        def _add(_fd, raw_path, _mask):
            add_calls.append(Path(os.fsdecode(raw_path)))
            return len(add_calls)

        inotify_add_watch = FakeFunction(_add)

    original_rglob = Path.rglob

    def asserting_rglob(path: Path, pattern: str):
        assert path == root
        assert add_calls == [root]
        return original_rglob(path, pattern)

    monkeypatch.setattr(execution_custody.ctypes, "CDLL", lambda *_a, **_k: FakeLibc())
    monkeypatch.setattr(Path, "rglob", asserting_rglob)
    monkeypatch.setattr(execution_custody.os, "O_NONBLOCK", 0x800, raising=False)
    monkeypatch.setattr(execution_custody.os, "O_CLOEXEC", 0x80000, raising=False)
    monkeypatch.setattr(
        execution_custody.os,
        "read",
        lambda *_a, **_k: (_ for _ in ()).throw(BlockingIOError()),
    )
    monitor._stop.set()

    monitor._run_linux()

    assert add_calls == [root]
    assert monitor._ready.is_set()
