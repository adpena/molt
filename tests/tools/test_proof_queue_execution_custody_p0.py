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


@pytest.mark.skipif(os.name == "nt", reason="CPython POSIX exec-path contract")
@pytest.mark.parametrize(
    "selection",
    [
        "inherit",
        "missing",
        "empty",
        "relative",
        "lowercase",
        "bytes-missing",
        "bytes-empty",
        "bytes-lowercase",
    ],
)
def test_python_hook_broker_matches_cpython_child_path_selection(
    tmp_path: Path, selection: str
) -> None:
    # Use real native interpreter aliases: the independent ordinary CPython
    # launch is the oracle, and the broker sees the actual bootstrap hook.
    image = Path(sys.executable).resolve(strict=True)
    name = "molt-custody-child-" + tmp_path.name
    parent_bin = tmp_path / "parent-bin"
    child_cwd = tmp_path / "child-cwd"
    for directory in (parent_bin, child_cwd, child_cwd / "bin"):
        directory.mkdir(parents=True, exist_ok=True)
        (directory / name).symlink_to(image)
    child_environment = {
        "inherit": None,
        "missing": {},
        "empty": {"PATH": ""},
        "relative": {"PATH": "bin"},
        "lowercase": {"path": str(parent_bin)},
        "bytes-missing": {b"OTHER": b"present"},
        "bytes-empty": {b"PATH": b""},
        "bytes-lowercase": {b"path": os.fsencode(parent_bin)},
    }[selection]
    payload = (
        "import json, subprocess\n"
        "try:\n"
        f" result = subprocess.run([{name!r}, '-I', '-S', '-c', "
        "\"print('selected-native-child')\"], "
        f"env={child_environment!r}, cwd={str(child_cwd)!r}, "
        "capture_output=True, text=True, timeout=10)\n"
        " print(json.dumps({'returncode': result.returncode, 'stdout': result.stdout}))\n"
        "except OSError as exc:\n"
        " print(json.dumps({'error': type(exc).__name__}))\n"
    )
    environment = {
        name: value
        for name, value in os.environ.items()
        if not name.startswith("MOLT_PROOF_CHILD_CUSTODY")
    }
    environment["PATH"] = str(parent_bin)
    oracle = run_custody_subject_process(
        [sys.executable, "-I", "-S", "-c", payload],
        env=environment,
        check=False,
        capture_output=True,
        text=True,
        timeout=20,
    )
    assert oracle.returncode == 0, oracle.stderr
    expected = json.loads(oracle.stdout)
    found = selection in {"inherit", "empty", "relative", "bytes-empty"}
    assert expected == (
        {"returncode": 0, "stdout": "selected-native-child\n"}
        if found
        else {"error": "FileNotFoundError"}
    )
    policy = {
        "schema": execution_custody.CHILD_POLICY_SCHEMA,
        "descendants": "declared-toolchains",
        "allowed": [
            {
                "toolchain": "python",
                "path": execution_custody._norm(directory / name),
                "sha256": hashlib.sha256(image.read_bytes()).hexdigest(),
            }
            for directory in (parent_bin, child_cwd, child_cwd / "bin")
        ],
    }
    server = execution_custody.ChildCustodyEventServer("python", policy)
    environment[execution_custody.CHILD_POLICY_ENV] = json.dumps(policy)
    environment.update(server.environment())
    bootstrap = Path(execution_custody.__file__).with_name(
        "python_custody_bootstrap.py"
    )
    with server:
        completed = run_custody_subject_process(
            [sys.executable, bootstrap, "command", "0", payload],
            env=environment,
            check=False,
            capture_output=True,
            text=True,
            timeout=20,
        )
    assert completed.returncode == 0, completed.stderr
    assert json.loads(completed.stdout) == (
        expected if found else {"error": "PermissionError"}
    )
    receipt = server.receipt()
    assert receipt["broker_complete"] is True, receipt
    assert receipt["errors"] == [], receipt
    decisions = [
        row for row in receipt["events"] if row.get("event") == "child-process"
    ]
    assert len(decisions) == 1, receipt
    assert decisions[0]["admitted"] is found, decisions
    selected_directory = (
        parent_bin
        if selection == "inherit"
        else child_cwd / "bin"
        if selection == "relative"
        else child_cwd
    )
    assert decisions[0]["resolved"] == (
        str(selected_directory / name) if found else None
    ), decisions
    assert bool(receipt["violations"]) is not found


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
    policy = execution_custody.child_policy(
        envelope, {}, environment_executables={}, derived_roots=provenance
    )
    role = supervisor_custody.SCRATCH_OUTPUT_ROLE
    assert policy["derived_roots"] == [{"role": role, "path": str(scratch.resolve())}]
    with pytest.raises(ValueError, match="run-owned provenance"):
        execution_custody.child_policy(
            envelope,
            {},
            environment_executables={},
            derived_roots=[{**provenance[0], "run_owned": False}],
        )
    with pytest.raises(ValueError, match="run-owned provenance"):
        execution_custody.child_policy(
            {"process_closure": {"descendants": "forbidden"}},
            {},
            environment_executables={},
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
        environment_executables={},
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
        json.dumps({"event": {"kind": "exec", "image": image}}) + "\n", encoding="utf-8"
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


@pytest.mark.skipif(os.name == "nt", reason="POSIX native interpreter alias execution")
def test_python_path_and_cargo_hook_launches_share_native_environment_custody(tmp_path):
    import shutil
    from tools import proof_plan
    from tools.proof_queue_pkg import (
        command_admission,
        command_identity,
        execution_environment,
    )

    path_bin = tmp_path / "path"
    path_bin.mkdir()
    path_tool, hook_tool = path_bin / "cargo", tmp_path / "selected-cargo"
    for image in (path_tool, hook_tool):
        shutil.copyfile(Path(sys.executable).resolve(strict=True), image)
        image.chmod(0o755)
    (tmp_path / "pyvenv.cfg").write_text(
        f"home = {Path(sys._base_executable).resolve(strict=True).parent}\n",
        encoding="utf-8",
    )
    environment = {**os.environ, "PATH": str(path_bin), "CARGO": str(hook_tool)}
    environment, _ = execution_environment._deterministic_execution_environment(
        environment, override_names=["CARGO"]
    )
    envelope = command_admission.envelope_for_command([sys.executable, "-c", "pass"])
    identity = command_identity._tool_identity(
        proof_plan.ProofPlan.load(),
        "cargo",
        envelope,
        [sys.executable, "-c", "pass"],
        cwd=tmp_path,
        env=environment,
    )
    assert identity["path"] == str(path_tool)
    configured = execution_environment._execution_environment_executable_identities(
        environment, cwd=tmp_path
    )
    declared = {"process_closure": {"descendants": "declared-toolchains"}}
    policy = execution_custody.child_policy(
        declared, {"cargo": identity}, environment_executables=configured
    )
    _, fixed = supervisor_custody._supervisor_fixed_images(
        {"cargo": identity}, configured, [sys.executable]
    )
    wanted = {str(path_tool), str(hook_tool)}
    assert wanted <= {row["path"] for row in policy["allowed"]}
    assert wanted <= {row["path"] for row in fixed}
    payload = (
        "import os, subprocess\n"
        "for executable in ('cargo', os.environ['CARGO']):\n"
        " subprocess.run([executable, '-I', '-S', '-c', \"print('actual-child')\"], check=True)\n"
    )
    server = execution_custody.ChildCustodyEventServer("python", policy)
    environment[execution_custody.CHILD_POLICY_ENV] = json.dumps(policy)
    environment.update(server.environment())
    bootstrap = Path(execution_custody.__file__).with_name(
        "python_custody_bootstrap.py"
    )
    with server:
        completed = run_custody_subject_process(
            [sys.executable, bootstrap, "command", "0", payload],
            env=environment,
            cwd=tmp_path,
            check=False,
            capture_output=True,
            text=True,
            timeout=20,
        )
    assert completed.returncode == 0, completed.stderr
    assert completed.stdout.splitlines() == ["actual-child", "actual-child"]
    receipt = server.receipt()
    assert execution_custody.child_receipt_is_admitted(receipt), receipt
    assert {
        row["resolved"]
        for row in receipt["events"]
        if row.get("event") == "child-process"
    } == wanted
    # The recorded allowance binds bytes, rather than granting a directory.
    hook_tool.write_bytes(b"replacement")
    with execution_custody.ChildCustodyEventServer("python", policy) as changed:
        assert not changed._decide_child({"requested": str(hook_tool)})["admitted"]


@pytest.mark.parametrize(
    "selector",
    [
        "CARGO",
        "RUSTC",
        "RUSTFMT",
        "RUSTDOC",
        "CARGO_BUILD_RUSTC",
        "CARGO_BUILD_RUSTDOC",
    ],
)
def test_environment_rust_proxy_component_reaches_both_custody_consumers(
    tmp_path, monkeypatch, selector
):
    import subprocess
    from molt import rust_toolchain
    from tools.proof_queue_pkg import (
        execution_environment,
        toolchain_capture,
        process_image_capture,
    )

    role = selector.removeprefix("CARGO_BUILD_").lower()
    proxy = tmp_path / (role + (".exe" if os.name == "nt" else ""))
    rustup = proxy.with_name("rustup.exe" if os.name == "nt" else "rustup")
    physical = tmp_path / "physical" / proxy.name
    physical.parent.mkdir()
    for path in (proxy, rustup, physical):
        path.write_bytes(b"proxy" if path != physical else b"actual component")
        path.chmod(0o755)
    calls = []

    def which(command, **kwargs):
        calls.append(command)
        return subprocess.CompletedProcess(command, 0, str(physical) + "\n", "")

    monkeypatch.setattr(rust_toolchain.process_guard, "run_completed_command", which)
    captured = execution_environment._execution_environment_executable_identities(
        {selector: str(proxy)}, cwd=tmp_path
    )
    assert calls == [[str(rustup), "which", role]]
    images = process_image_capture.environment_images(captured)
    assert {row["path"] for row in images} == {str(proxy), str(physical)}
    assert {row.path for row in toolchain_capture.frozen_files(captured)} >= {
        str(proxy),
        str(physical),
    }
    assert set(execution_custody._identity_paths(captured)) >= {proxy, physical}
    policy = execution_custody.child_policy(
        {"process_closure": {"descendants": "declared-toolchains"}},
        {},
        environment_executables=captured,
    )
    _, native = supervisor_custody._supervisor_fixed_images(
        {}, captured, [sys.executable]
    )
    expected = {(row["path"], row["sha256"]) for row in images}
    assert {(row["path"], row["sha256"]) for row in policy["allowed"]} == expected
    assert {
        (row["path"], row["sha256"])
        for row in native
        if row["role"] == f"env:{selector}"
    } == expected
    physical.write_bytes(b"changed component")
    assert (
        execution_environment._execution_environment_executable_identities(
            {selector: str(proxy)}, cwd=tmp_path
        )
        != captured
    )
    with pytest.raises(ValueError, match="changed while live custody"):
        process_image_capture.revalidate_images(images)
    incomplete = {
        selector: {
            key: value
            for key, value in captured[selector].items()
            if key != "process_images"
        }
    }
    with pytest.raises(ValueError, match="no process-image closure"):
        execution_custody.child_policy(
            {"process_closure": {"descendants": "declared-toolchains"}},
            {},
            environment_executables=incomplete,
        )
    with pytest.raises(ValueError, match="no process-image closure"):
        supervisor_custody._supervisor_fixed_images({}, incomplete, [sys.executable])


def test_watch_custody_retains_ancestor_alias_and_deleted_entry(tmp_path):
    source = tmp_path / "source"
    source.mkdir()
    target = tmp_path / "target"
    target.mkdir()
    tool = target / "tool"
    tool.write_bytes(b"same bytes")
    alias = tmp_path / "alias"
    try:
        alias.symlink_to(target, target_is_directory=True)
    except OSError as exc:
        pytest.skip(f"directory symlink capability unavailable: {exc}")
    selected = alias / "tool"
    specs = execution_custody.watch_specs(
        source_root=source,
        tracked_paths=[],
        identities=[{"path": str(selected)}],
        broad_roots=[],
    )
    assert any(spec.owns(tool) for spec in specs if spec.root == target)
    alias_spec = next(spec for spec in specs if spec.root == tmp_path)
    assert alias_spec.owns(alias)
    alias.unlink()
    assert alias_spec.owns(alias), "removed selection must retain a mutation event"
    replacement = tmp_path / "replacement"
    replacement.mkdir()
    (replacement / "tool").write_bytes(tool.read_bytes())
    alias.symlink_to(replacement, target_is_directory=True)
    assert alias_spec.owns(alias), "same-byte alias retarget still changes selection"
