from __future__ import annotations

import contextlib
import os
import sys
import json
import queue
import threading
import time

import pytest

from tools import batch_compile_client
from tools.batch_compile_client import (
    BatchCompileProtocolError,
    BatchCompileServerClient,
)
import subprocess
from tests.process_guard_common import install_module_view
from molt.cli.build_request import BuildRequest, build_request_to_payload
from molt.process_environment import NameRule, ProcessEnvironment


class _FakeProc:
    def __init__(self) -> None:
        read_fd, write_fd = os.pipe()
        self.stdin = os.fdopen(write_fd, "w")
        self.sent = os.fdopen(read_fd, "rb", buffering=0)
        os.set_blocking(write_fd, False)
        self.stderr = None

    def poll(self) -> None:
        return None


@pytest.fixture
def bare_client():
    client = BatchCompileServerClient.__new__(BatchCompileServerClient)
    client._proc = _FakeProc()
    client._request_lock = threading.Lock()
    client._closing = threading.Event()
    client._stderr_tail = b""
    client._stderr_error = None
    client._next_id = 1
    client._poisoned = False
    client._response_queue = queue.Queue()
    client.child_process = (
        batch_compile_client.harness_memory_guard.memory_guard.GuardedChildProcess(
            11, 11, 11, ("fixture",), "fixture", 1000
        )
    )
    try:
        yield client
    finally:
        client._proc.stdin.close()
        client._proc.sent.close()


@pytest.mark.parametrize("rule", [NameRule.EXACT, NameRule.WINDOWS])
def test_batch_build_envelope_keeps_environment_outside_typed_params(
    monkeypatch, rule, bare_client
) -> None:
    client = bare_client
    monkeypatch.setattr(
        client, "_readline", lambda deadline, **kwargs: '{"id": 1, "ok": true}'
    )
    params = build_request_to_payload(
        BuildRequest(file_path="program.py", profile="dev", parse_codec="json")
    )
    environment = ProcessEnvironment(
        {"Path": "/fixture/bin", "MOLT_CODEC": "json"}, rule=rule
    )

    client.request("build", params=params, env_overrides=environment, timeout=1.0)

    sent = json.loads(client._proc.sent.readline())
    assert sent["id"] == 1 and sent["op"] == "build"
    assert sent["env_overrides"] == {"Path": "/fixture/bin", "MOLT_CODEC": "json"}
    assert sent["params"] == params
    assert sent["params"]["profile"] == "dev"
    assert sent["params"]["parse_codec"] == "json"
    assert "env_overrides" not in sent["params"]


@pytest.mark.parametrize(
    ("raw", "actual_id"),
    [
        ('{"id": 2, "ok": true}', 2),
        ('{"id": true, "ok": true}', True),
        ('{"id": 1.0, "ok": true}', 1.0),
        ('{"id": "1", "ok": true}', "1"),
        ('{"id": null, "ok": true}', None),
    ],
)
def test_batch_compile_client_rejects_response_id_mismatch(
    monkeypatch, bare_client, raw, actual_id
) -> None:
    client = bare_client
    monkeypatch.setattr(client, "_readline", lambda deadline, **kwargs: raw)

    with pytest.raises(BatchCompileProtocolError) as exc_info:
        client.request("ping", timeout=1.0)
    message = str(exc_info.value)
    assert "batch compile response id mismatch" in message
    assert "expected id 1" in message
    if actual_id is not None:
        assert f"got id {actual_id!r}" in message
    assert "op 'ping'" in message
    assert raw in message
    assert type(exc_info.value.actual_id) is type(actual_id)
    assert exc_info.value.actual_id == actual_id
    assert exc_info.value.batch_request_custody.child_process == client.child_process
    assert exc_info.value.batch_request_custody.returncode == 127
    assert client._poisoned is True
    assert json.loads(client._proc.sent.readline())["id"] == 1
    with pytest.raises(RuntimeError, match="restart required"):
        client.request("ping", timeout=1.0)
    assert client._next_id == 2


def test_batch_compile_client_requires_restart_after_response_timeout(
    monkeypatch,
    bare_client,
) -> None:
    client = bare_client

    def _timeout(deadline, **kwargs):
        del deadline
        raise TimeoutError("batch compile server response timed out")

    monkeypatch.setattr(client, "_readline", _timeout)

    with pytest.raises(TimeoutError, match="response timed out"):
        client.request("ping", timeout=1.0)
    with pytest.raises(RuntimeError, match="restart required"):
        client.request("ping", timeout=1.0)


def test_batch_compile_client_readline_timeout_is_bounded(bare_client) -> None:
    client = bare_client

    with pytest.raises(TimeoutError, match="response timed out"):
        client._readline(time.monotonic() + 0.01)


def test_batch_request_custody_is_immutable_across_responses_and_errors(
    monkeypatch, bare_client
):
    client = bare_client
    times = iter((100, 200, 300))
    install_module_view(
        monkeypatch,
        "time",
        time,
        batch_compile_client,
        monotonic_ns=lambda: next(times),
    )
    replies = iter(
        (
            '{"id": 1, "ok": true, "returncode": 0}',
            '{"id": 2, "ok": false, "returncode": 5}',
        )
    )
    monkeypatch.setattr(client, "_readline", lambda deadline, **kwargs: next(replies))
    first = client.request("build", timeout=1)
    second = client.request("build", timeout=1)
    assert first.custody.request_started_at_ns == 100
    assert second.custody.request_started_at_ns == 200
    assert first.custody.returncode == 0
    assert second.custody.returncode == 5

    def closed(deadline, **kwargs):
        raise RuntimeError("server died before response")

    monkeypatch.setattr(client, "_readline", closed)
    with pytest.raises(RuntimeError) as error:
        client.request("build", timeout=1)
    assert error.value.batch_request_custody.request_started_at_ns == 300
    assert error.value.batch_request_custody.child_process.started_at_ns == 1000
    assert first.custody.request_started_at_ns == 100


def test_batch_compile_client_owns_guard_context_by_default(
    monkeypatch,
    tmp_path,
) -> None:
    events: list[tuple[str, object]] = []

    class FakeSentinel:
        def __exit__(self, exc_type, exc, tb) -> None:
            events.append(("sentinel_exit", exc_type))

    class FakeContext:
        env = {"PATH": "/usr/bin", "TMPDIR": str(tmp_path / "tmp")}

        class Limits:
            enabled = True

        limits = Limits()

        def process_group_kwargs(self) -> dict[str, object]:
            events.append(("process_group_kwargs", True))
            return {"start_new_session": True}

        def force_close_process_group(self, proc) -> None:
            events.append(("force_close", proc))

        def start_repo_sentinel(self, *, label: str, **kwargs):
            events.append(("sentinel_label", label))
            return FakeSentinel()

    def fake_from_env(prefix, env, *, repo_root):
        events.append(("from_env", (prefix, dict(env), repo_root)))
        return FakeContext()

    class FakeProc:
        def __init__(self, *args, **kwargs) -> None:
            events.append(("popen_kwargs", kwargs))
            self.pid = 12345
            read_fd, write_fd = os.pipe()
            os.close(read_fd)
            self.stdin = os.fdopen(write_fd, "w")
            read_fd, write_fd = os.pipe()
            os.close(write_fd)
            self.stdout = os.fdopen(read_fd, "r")
            read_fd, write_fd = os.pipe()
            os.close(write_fd)
            self.stderr = os.fdopen(read_fd, "r")

        def poll(self) -> int | None:
            return 0

        def terminate(self) -> None:
            events.append(("terminate", True))

        def wait(self, timeout=None) -> int:
            return 0

    monkeypatch.setattr(
        batch_compile_client.harness_memory_guard.HarnessExecutionContext,
        "from_env",
        fake_from_env,
    )
    install_module_view(
        monkeypatch, "subprocess", subprocess, batch_compile_client, Popen=FakeProc
    )
    guard = batch_compile_client.harness_memory_guard.memory_guard
    monkeypatch.setattr(guard._process_model, "process_started_at_ns", lambda pid: 1000)
    monkeypatch.setattr(
        guard, "windows_process_handle_started_at_ns", lambda handle: 1000
    )
    monkeypatch.setattr(guard, "_safe_getpgid", lambda pid: pid)
    monkeypatch.setattr(guard, "_safe_getsid", lambda pid: pid)

    client = BatchCompileServerClient(
        ["molt", "internal-batch-build-server"],
        cwd=tmp_path,
        env={"PATH": "/usr/bin"},
    )
    client.close(force=True)

    assert events[0][0] == "from_env"
    assert ("process_group_kwargs", True) in events
    assert ("sentinel_label", "molt_batch_server") in events
    popen_kwargs = next(value for key, value in events if key == "popen_kwargs")
    assert popen_kwargs["env"]["TMPDIR"] == str(tmp_path / "tmp")
    assert popen_kwargs["start_new_session"] is True
    assert any(key == "force_close" for key, _ in events)
    assert ("sentinel_exit", None) in events


def _managed_batch_environment(tmp_path, rule=NameRule.EXACT):
    import os
    from molt.dx import checkout_component
    from molt.build_state_layout import build_state_root

    checkout = tmp_path / "project"
    checkout.mkdir()
    artifact = tmp_path / "artifacts"
    parent = artifact / "target" / "checkouts" / checkout_component(checkout)
    session = parent / "sessions" / "reader"
    key = "cargo_target_dir" if rule is NameRule.WINDOWS else "CARGO_TARGET_DIR"
    environment = ProcessEnvironment(
        {**os.environ, key: str(session), "MOLT_EXT_ROOT": str(artifact)}, rule=rule
    )
    control = build_state_root(
        project_root=checkout, cargo_target=session, environment=environment
    )
    script = tmp_path / "server.py"
    script.write_text(
        "import json,sys\n"
        "for line in sys.stdin:\n"
        " request=json.loads(line)\n"
        " print(json.dumps({'id':request['id'],'ok':True}),flush=True)\n"
        " if request['op']=='shutdown': break\n",
        encoding="utf-8",
    )
    return checkout, environment, (parent, session, control), script


def _assert_batch_retirement(roots, available):
    from molt.checkout_state import checkout_state_retirement

    for root in roots:
        with checkout_state_retirement(root) as admitted:
            assert admitted is available, root


@pytest.mark.parametrize("rule", list(NameRule))
def test_persistent_batch_child_holds_actual_roots_until_closed(tmp_path, rule):
    import sys

    checkout, environment, roots, script = _managed_batch_environment(tmp_path, rule)
    client = BatchCompileServerClient(
        [sys.executable, str(script)],
        cwd=checkout,
        env=environment,
        memory_guard_prefix=None,
    )
    try:
        _assert_batch_retirement(roots, False)
        assert client.request("ping", timeout=5)["ok"]
        _assert_batch_retirement(roots, False)
    finally:
        client.close(timeout=5)
    assert client._proc.poll() is not None
    _assert_batch_retirement(roots, True)


def test_batch_lifetime_uses_compiler_source_independently_of_server_cwd(
    tmp_path, monkeypatch
):
    import sys
    from pathlib import Path
    from molt import checkout_state
    from molt.build_state_layout import build_state_root

    checkout, environment, roots, script = _managed_batch_environment(tmp_path)
    compiler = tmp_path / "compiler-source"
    compiler.mkdir()
    environment = ProcessEnvironment(
        {key: value for key, value in environment.items() if key != "MOLT_EXT_ROOT"}
    )
    assert "MOLT_EXT_ROOT" not in environment
    monkeypatch.setattr(batch_compile_client, "compiler_source_root", lambda: compiler)
    original = checkout_state.checkout_state_command_roots
    captured = []

    def observe(source_root, child_environment, command, *, cwd):
        assert source_root == compiler and cwd == checkout
        assert command == [sys.executable, str(script)]
        selected = original(source_root, child_environment, command, cwd=cwd)
        target = Path(child_environment["CARGO_TARGET_DIR"])
        assert (
            build_state_root(
                project_root=compiler,
                cargo_target=target,
                environment=child_environment,
            ),
            checkout_state.CheckoutOwner(target, "build-control"),
        ) in selected
        captured.append(selected)
        return selected

    monkeypatch.setattr(checkout_state, "checkout_state_command_roots", observe)
    client = BatchCompileServerClient(
        [sys.executable, str(script)],
        cwd=checkout,
        env=environment,
        memory_guard_prefix=None,
    )
    try:
        assert captured and client.request("ping", timeout=5)["ok"]
        _assert_batch_retirement(roots[:2], False)
    finally:
        client.close(timeout=5)
    _assert_batch_retirement(roots[:2], True)


def test_batch_spawn_failure_releases_prebirth_use(tmp_path, monkeypatch):
    checkout, environment, roots, script = _managed_batch_environment(tmp_path)

    def refuse(*args, **kwargs):
        _assert_batch_retirement(roots, False)
        raise FileNotFoundError("independent batch launch refusal")

    install_module_view(
        monkeypatch, "subprocess", subprocess, batch_compile_client, Popen=refuse
    )
    with pytest.raises(FileNotFoundError, match="independent batch launch refusal"):
        BatchCompileServerClient(
            ["unused"], cwd=checkout, env=environment, memory_guard_prefix=None
        )
    _assert_batch_retirement(roots, True)


def test_batch_reader_start_failure_and_failed_cleanup_survive_gc(
    tmp_path, monkeypatch
):
    import gc
    import sys
    import weakref

    checkout, environment, roots, script = _managed_batch_environment(tmp_path)
    processes = []
    original_popen = subprocess.Popen

    def capture(*args, **kwargs):
        process = original_popen(*args, **kwargs)
        processes.append(process)
        return process

    class FailingReader(threading.Thread):
        def start(self):
            raise RuntimeError("independent batch reader start failure")

    install_module_view(
        monkeypatch, "subprocess", subprocess, batch_compile_client, Popen=capture
    )
    install_module_view(
        monkeypatch, "threading", threading, batch_compile_client, Thread=FailingReader
    )
    reference = None
    try:
        with pytest.raises(
            RuntimeError, match="independent batch reader start failure"
        ) as caught:
            BatchCompileServerClient(
                [sys.executable, str(script)],
                cwd=checkout,
                env=environment,
                memory_guard_prefix=None,
                force_close=lambda process: None,
            )
        reference = weakref.ref(caught.value.batch_client)
        del caught
        gc.collect()
        assert reference() is not None
        assert len(processes) == 1 and processes[0].poll() is None
        _assert_batch_retirement(roots, False)
    finally:
        client = reference() if reference is not None else None
        if client is not None:
            client._force_close = None
            client.close(force=True, timeout=5)
        else:
            for process in processes:
                process.terminate()
                process.wait(timeout=5)
    _assert_batch_retirement(roots, True)
    del client
    gc.collect()
    assert reference() is None


def test_batch_failed_force_close_retains_use_until_observed_death(tmp_path):
    import sys

    checkout, environment, roots, script = _managed_batch_environment(tmp_path)
    client = BatchCompileServerClient(
        [sys.executable, str(script)],
        cwd=checkout,
        env=environment,
        memory_guard_prefix=None,
        force_close=lambda process: None,
    )
    try:
        with pytest.raises(RuntimeError, match="child closure unresolved"):
            client.close(force=True)
        _assert_batch_retirement(roots, False)
    finally:
        client._force_close = None
        client.close(force=True, timeout=5)
    _assert_batch_retirement(roots, True)


@contextlib.contextmanager
def _script_client(tmp_path, source):
    checkout, environment, roots, script = _managed_batch_environment(tmp_path)
    script.write_text(source, encoding="utf-8")
    client = BatchCompileServerClient(
        [sys.executable, str(script)],
        cwd=checkout,
        env=environment,
        memory_guard_prefix=None,
    )
    try:
        yield client, roots
    finally:
        client.close(force=True, timeout=5)


def test_batch_request_deadline_includes_unread_stdin(tmp_path):
    with _script_client(tmp_path, "import time; time.sleep(60)\n") as (client, _):
        started = time.monotonic()
        with pytest.raises(TimeoutError, match="request write timed out") as caught:
            client.request(
                "build", params={"large": "x" * (2 * 1024 * 1024)}, timeout=0.1
            )
        assert time.monotonic() - started < 2
        assert caught.value.batch_request_custody.child_process == client.child_process
        assert client._poisoned
        with pytest.raises(RuntimeError, match="restart required"):
            client.request("ping", timeout=1)


def test_batch_stdout_eof_never_waits_for_stderr_eof(tmp_path):
    source = (
        "import os,sys,time\n"
        "sys.stdin.readline()\n"
        "os.write(2,b'independent stderr diagnostic\\n')\n"
        "os.close(1)\n"
        "time.sleep(60)\n"
    )
    with _script_client(tmp_path, source) as (client, _):
        started = time.monotonic()
        with pytest.raises(RuntimeError, match="closed response pipe"):
            client.request("ping", timeout=2)
        assert time.monotonic() - started < 3
        assert client._proc.poll() is None
        assert client._poisoned


def test_batch_stderr_flood_is_drained_with_bounded_tail(tmp_path):
    source = (
        "import json,os,sys\n"
        "request=json.loads(sys.stdin.readline())\n"
        "for _ in range(512): os.write(2,b'x'*4096)\n"
        "os.write(2,b'INDEPENDENT-TAIL')\n"
        "print(json.dumps({'id':request['id'],'ok':True}),flush=True)\n"
        "sys.stdin.readline()\n"
    )
    with _script_client(tmp_path, source) as (client, _):
        assert client.request("ping", timeout=5)["ok"]
        deadline = time.monotonic() + 2
        while (
            not client._stderr_tail.endswith(b"INDEPENDENT-TAIL")
            and time.monotonic() < deadline
        ):
            time.sleep(0.01)
        assert client._stderr_tail.endswith(b"INDEPENDENT-TAIL")
        assert len(client._stderr_tail) <= 8192


def test_batch_lock_timeout_has_no_custody_and_does_not_poison(bare_client):
    client = bare_client
    client._request_lock.acquire()
    try:
        started = time.monotonic()
        with pytest.raises(TimeoutError, match="admission timed out") as caught:
            client.request("ping", timeout=0.03)
        assert time.monotonic() - started < 1
        assert not hasattr(caught.value, "batch_request_custody")
        assert not client._poisoned
        assert client._next_id == 1
    finally:
        client._request_lock.release()


def test_batch_partial_write_failure_poisons_protocol(monkeypatch, bare_client):
    client = bare_client
    original_write = os.write
    calls = 0

    def partial_then_fail(fd, payload):
        nonlocal calls
        calls += 1
        if calls == 1:
            return original_write(fd, payload[:7])
        raise BrokenPipeError("independent partial-write failure")

    install_module_view(
        monkeypatch, "os", os, batch_compile_client, write=partial_then_fail
    )
    with pytest.raises(RuntimeError, match="independent partial-write failure"):
        client.request("ping", timeout=1)
    assert client._proc.sent.read(7) == b'{"id": '
    assert client._poisoned
    with pytest.raises(RuntimeError, match="restart required"):
        client.request("ping", timeout=1)
    assert calls == 2


def test_batch_one_deadline_covers_admission_write_and_response(
    monkeypatch, bare_client
):
    client = bare_client
    clock = [10.0]
    deadlines = []

    class Admission:
        def acquire(self, *, timeout):
            assert timeout == 1.0
            clock[0] += 0.4
            return True

        def release(self):
            pass

    def write(payload, deadline, **kwargs):
        deadlines.append(deadline)
        clock[0] += 0.4

    def read(deadline, **kwargs):
        deadlines.append(deadline)
        assert deadline - clock[0] == pytest.approx(0.2)
        return '{"id":1,"ok":true}'

    client._request_lock = Admission()
    install_module_view(
        monkeypatch, "time", time, batch_compile_client, monotonic=lambda: clock[0]
    )
    monkeypatch.setattr(client, "_write_request", write)
    monkeypatch.setattr(client, "_readline", read)
    assert client.request("ping", timeout=1)["ok"]
    assert deadlines == [11.0, 11.0]


def test_batch_force_close_interrupts_writer_before_releasing_use(
    tmp_path, monkeypatch
):
    with _script_client(tmp_path, "import time; time.sleep(60)\n") as (client, roots):
        blocked = threading.Event()
        stopped = threading.Event()
        original_write = os.write
        original_close = client.force_close
        failures = []

        def stop_child():
            original_close()
            stopped.set()

        monkeypatch.setattr(client, "force_close", stop_child)

        def observe(fd, payload):
            try:
                return original_write(fd, payload)
            except BlockingIOError:
                blocked.set()
                # Keep this writer's admission until the closer actually stops
                # the child. A closer waiting on the lock first cannot pass.
                assert stopped.wait(5)
                assert not client._proc.stdin.closed
                raise

        install_module_view(monkeypatch, "os", os, batch_compile_client, write=observe)

        def request():
            try:
                client.request(
                    "build", params={"large": "x" * (2 * 1024 * 1024)}, timeout=30
                )
            except Exception as exc:
                failures.append(exc)

        worker = threading.Thread(target=request, daemon=True)
        worker.start()
        try:
            assert blocked.wait(5)
            _assert_batch_retirement(roots, False)
            started = time.monotonic()
            client.close(force=True, timeout=2)
            assert time.monotonic() - started < 3
            worker.join(timeout=2)
            assert not worker.is_alive()
            assert failures and isinstance(failures[0], RuntimeError)
            assert (
                failures[0].batch_request_custody.child_process == client.child_process
            )
            assert client._poisoned
            assert client._proc.stdin.closed
            _assert_batch_retirement(roots, True)
        finally:
            client.close(force=True, timeout=5)
            worker.join(timeout=5)


def test_batch_unresolved_reader_retains_streams_and_use(tmp_path, monkeypatch):
    release = threading.Event()
    entered = threading.Event()
    original = BatchCompileServerClient._stderr_reader_loop

    def blocked_reader(self):
        entered.set()
        release.wait()
        original(self)

    monkeypatch.setattr(BatchCompileServerClient, "_stderr_reader_loop", blocked_reader)
    with _script_client(tmp_path, "import time; time.sleep(60)\n") as (client, roots):
        try:
            assert entered.wait(5)
            with pytest.raises(RuntimeError, match="reader closure unresolved"):
                client.close(force=True, timeout=0.03)
            assert client._proc.poll() is not None
            assert not client._proc.stdin.closed
            assert not client._proc.stdout.closed
            assert not client._proc.stderr.closed
            _assert_batch_retirement(roots, False)
        finally:
            release.set()
            client.close(force=True, timeout=5)
        _assert_batch_retirement(roots, True)


def test_batch_nonblocking_setup_failure_closes_child_and_use(tmp_path, monkeypatch):
    checkout, environment, roots, script = _managed_batch_environment(tmp_path)
    processes = []
    original_popen = subprocess.Popen

    def capture(*args, **kwargs):
        process = original_popen(*args, **kwargs)
        processes.append(process)
        return process

    def refuse(fd, blocking):
        assert blocking is False
        raise OSError("independent nonblocking setup refusal")

    install_module_view(
        monkeypatch, "subprocess", subprocess, batch_compile_client, Popen=capture
    )
    install_module_view(
        monkeypatch, "os", os, batch_compile_client, set_blocking=refuse
    )
    with pytest.raises(OSError, match="independent nonblocking setup refusal"):
        BatchCompileServerClient(
            [sys.executable, str(script)],
            cwd=checkout,
            env=environment,
            memory_guard_prefix=None,
        )
    assert len(processes) == 1 and processes[0].poll() is not None
    assert all(
        stream.closed
        for stream in (processes[0].stdin, processes[0].stdout, processes[0].stderr)
    )
    _assert_batch_retirement(roots, True)


def test_batch_partial_writes_preserve_ascii_json_and_unicode(monkeypatch, bare_client):
    client = bare_client
    original_write = os.write
    pieces = []

    def write_piece(fd, payload):
        piece = bytes(payload[:3])
        assert piece.isascii()
        written = original_write(fd, piece)
        pieces.append(piece[:written])
        return written

    install_module_view(monkeypatch, "os", os, batch_compile_client, write=write_piece)
    monkeypatch.setattr(
        client, "_readline", lambda deadline, **kwargs: '{"id":1,"ok":true}'
    )
    assert client.request("ping", params={"text": "λ🐍\n"}, timeout=1)["ok"]
    sent = client._proc.sent.readline()
    assert len(pieces) > 1 and sent == b"".join(pieces)
    assert json.loads(sent)["params"] == {"text": "λ🐍\n"}


def test_batch_closing_after_lock_admission_has_no_request_custody(bare_client):
    client = bare_client
    released = []

    class Admission:
        def acquire(self, *, timeout):
            client._closing.set()
            return True

        def release(self):
            released.append(True)

    client._request_lock = Admission()
    with pytest.raises(RuntimeError, match="is closing") as caught:
        client.request("ping", timeout=1)
    assert not hasattr(caught.value, "batch_request_custody")
    assert not client._poisoned and client._next_id == 1
    assert released == [True]


def test_batch_graceful_close_sends_internal_shutdown(tmp_path):
    marker = tmp_path / "shutdown-observed"
    source = (
        "import json,pathlib,sys\n"
        "for line in sys.stdin:\n"
        " request=json.loads(line)\n"
        " if request['op']=='shutdown':\n"
        f"  pathlib.Path({str(marker)!r}).write_text('shutdown')\n"
        " print(json.dumps({'id':request['id'],'ok':True}),flush=True)\n"
        " if request['op']=='shutdown': break\n"
    )
    with _script_client(tmp_path, source) as (client, roots):
        assert client.request("ping", timeout=5)["ok"]
        client.close(timeout=5)
        assert marker.read_text() == "shutdown"
        _assert_batch_retirement(roots, True)
        with pytest.raises(RuntimeError, match="is closing") as caught:
            client.request("ping", timeout=1)
        assert not hasattr(caught.value, "batch_request_custody")


@pytest.mark.parametrize(("timeout", "admission_delay"), [(0.0, 0.0), (1.0, 1.0)])
def test_batch_expired_at_lock_admission_does_not_encode_or_write(
    monkeypatch, bare_client, timeout, admission_delay
):
    client = bare_client
    clock = [10.0]

    class Admission:
        def acquire(self, *, timeout):
            clock[0] += admission_delay
            return True

        def release(self):
            pass

    def forbidden(*args, **kwargs):
        pytest.fail("an expired request must not encode or write protocol bytes")

    client._request_lock = Admission()
    install_module_view(
        monkeypatch, "time", time, batch_compile_client, monotonic=lambda: clock[0]
    )
    install_module_view(
        monkeypatch, "json", json, batch_compile_client, dumps=forbidden
    )
    install_module_view(monkeypatch, "os", os, batch_compile_client, write=forbidden)
    with pytest.raises(TimeoutError, match="request timed out"):
        client.request("build", params={"large": "unused"}, timeout=timeout)
    assert client._next_id == 1
    assert not client._poisoned
