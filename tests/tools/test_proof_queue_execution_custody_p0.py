from __future__ import annotations

import json
import os
import runpy
import subprocess
import sys
import time
from pathlib import Path

import pytest

from tools.proof_queue_pkg import execution_custody


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
        "schema": "molt.proof-child-custody.v1",
        "descendants": "forbidden",
        "allowed": [],
    }
    server = execution_custody.ChildCustodyEventServer("python", policy)
    environment = dict(os.environ)
    environment[execution_custody.CHILD_POLICY_ENV] = json.dumps(policy)
    environment.update(server.environment())

    with server:
        completed = subprocess.run(
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
        Path(str(event["path"])).name == "transient.py"
        for event in receipt["events"]
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
