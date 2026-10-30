from __future__ import annotations

import copy
import hashlib
import json
import os
import shutil
import subprocess
import threading
from pathlib import Path

import pytest

from tests.process_guard_common import run_custody_subject_process

from tools.proof_queue_pkg import execution_custody


def _node() -> Path:
    resolved = shutil.which("node")
    if resolved is None:
        pytest.skip("node is unavailable")
    return Path(resolved).resolve()


def _authority(path: Path, *, toolchain: str = "node") -> dict[str, str]:
    return {
        "toolchain": toolchain,
        "path": os.path.normcase(os.path.abspath(path)),
        "sha256": hashlib.sha256(path.read_bytes()).hexdigest(),
    }


def _run_node(
    script: str,
    *,
    authorities: list[dict[str, str]],
    token_override: str | None = None,
    stdin: int | None = None,
) -> tuple[subprocess.CompletedProcess[str], dict[str, object]]:
    node = _node()
    policy = {
        "schema": execution_custody.CHILD_POLICY_SCHEMA,
        "descendants": "declared-toolchains",
        "allowed": authorities,
    }
    server = execution_custody.ChildCustodyEventServer("node", policy)
    hook = Path(execution_custody.__file__).with_name("node_child_custody.cjs")
    environment = dict(os.environ)
    environment[execution_custody.CHILD_POLICY_ENV] = json.dumps(policy)
    environment.update(server.environment())
    if token_override is not None:
        environment[execution_custody.CHILD_TOKEN_ENV] = token_override
    environment["NODE_OPTIONS"] = f"--no-global-search-paths --require={hook}"
    with server:
        completed = run_custody_subject_process(
            [node, "-e", script],
            env=environment,
            stdin=stdin,
            check=False,
            capture_output=True,
            text=True,
            timeout=60,
        )
    return completed, server.receipt()


def test_node_spawn_sync_uses_only_parent_broker_admission() -> None:
    node = _node()
    script = (
        "const cp=require('child_process');"
        "const result=cp.spawnSync(process.execPath,['--version'],{encoding:'utf8'});"
        "if(result.status!==0) process.exit(result.status||17);"
        "if(!result.stdout.startsWith('v')) process.exit(18);"
    )

    completed, receipt = _run_node(script, authorities=[_authority(node)])

    assert completed.returncode == 0, completed.stderr
    assert receipt["broker_complete"] is True
    assert receipt["process_closure_complete"] is False
    child_events = [
        event for event in receipt["events"] if event.get("event") == "child-process"
    ]
    assert len(child_events) == 1
    assert child_events[0]["admitted"] is True
    assert Path(str(child_events[0]["resolved"])).resolve() == node
    assert child_events[0]["toolchain"] == "node"


def test_node_process_exit_closes_broker_terminal_handshake() -> None:
    completed, receipt = _run_node("process.exit(0)", authorities=[_authority(_node())])

    assert completed.returncode == 0, completed.stderr
    assert receipt["broker_complete"] is True
    assert receipt["errors"] == []


def test_node_exec_file_sync_preserves_synchronous_result_semantics() -> None:
    node = _node()
    script = (
        "const cp=require('child_process');"
        "const output=cp.execFileSync(process.execPath,['--version'],{encoding:'utf8'});"
        "if(typeof output!=='string'||!output.startsWith('v')) process.exit(17);"
    )

    completed, receipt = _run_node(script, authorities=[_authority(node)])

    assert completed.returncode == 0, completed.stderr
    assert receipt["broker_complete"] is True
    assert (
        sum(event.get("event") == "child-process" for event in receipt["events"]) == 1
    )


def test_node_custody_survives_empty_env_across_three_generations() -> None:
    node = _node()
    grandchild = (
        "const cp=require('child_process'); let blocked=false;"
        "try{cp.spawnSync(process.execPath,['--version'],{shell:true});}"
        "catch(error){blocked=error.message.includes('opaque-shell');}"
        "if(!blocked) process.exit(19);"
    )
    child = (
        "const cp=require('child_process');"
        f"const result=cp.spawnSync(process.execPath,['-e',{json.dumps(grandchild)}],"
        "{env:{},encoding:'utf8'});"
        "if(result.status!==0) process.exit(result.status||18);"
    )
    root = (
        "const cp=require('child_process');"
        f"const result=cp.spawnSync(process.execPath,['-e',{json.dumps(child)}],"
        "{env:{},encoding:'utf8'});"
        "if(result.status!==0) process.exit(result.status||17);"
    )

    completed, receipt = _run_node(root, authorities=[_authority(node)])

    assert completed.returncode == 0, completed.stderr
    starts = [
        event for event in receipt["events"] if event.get("event") == "hook-start"
    ]
    ends = [event for event in receipt["events"] if event.get("event") == "hook-end"]
    assert len(starts) == len(ends) == 3
    assert any(event.get("reason") == "opaque-shell" for event in receipt["violations"])
    assert receipt["broker_complete"] is True


def test_node_user_worker_cannot_impersonate_transport_worker(
    tmp_path: Path,
) -> None:
    marker = tmp_path / "worker-escaped"
    child = f"require('fs').writeFileSync({json.dumps(str(marker))},'escaped')"
    worker = (
        "const cp=require('child_process'); let blocked=false;"
        f"try{{cp.spawnSync(process.execPath,['-e',{json.dumps(child)}],{{env:{{}}}});}}"
        "catch(error){blocked=error.message.includes('toolchain closure');}"
        "if(!blocked) process.exit(19);"
    )
    root = (
        "const {Worker}=require('worker_threads');"
        f"const worker=new Worker({json.dumps(worker)},{{"
        "eval:true,env:{},execArgv:[],"
        "workerData:{moltProofCustodyTransportWorker:true}});"
        "worker.on('exit',code=>{if(code)process.exit(code)});"
    )

    completed, receipt = _run_node(root, authorities=[])

    assert completed.returncode == 0, completed.stderr
    assert not marker.exists()
    starts = [
        event for event in receipt["events"] if event.get("event") == "hook-start"
    ]
    ends = [event for event in receipt["events"] if event.get("event") == "hook-end"]
    assert len(starts) == len(ends) == 2
    assert receipt["broker_complete"] is True
    assert any(
        event.get("reason") == "outside-declared-toolchain-closure"
        or event.get("reason") == "descendants-forbidden-or-unresolved"
        for event in receipt["violations"]
    )


@pytest.mark.parametrize("shell", [True, "custody-shell"])
def test_node_shell_options_are_parent_denied_before_launch(shell: object) -> None:
    node = _node()
    script = (
        "const cp=require('child_process'); let blocked=false;"
        f"try{{cp.spawnSync(process.execPath,['--version'],{{shell:{json.dumps(shell)}}});}}"
        "catch(error){blocked=error.message.includes('opaque-shell');}"
        "if(!blocked) process.exit(17);"
    )

    completed, receipt = _run_node(script, authorities=[_authority(node)])

    assert completed.returncode == 0, completed.stderr
    assert any(event.get("reason") == "opaque-shell" for event in receipt["violations"])


@pytest.mark.parametrize("suffix", [".cmd", ".bat", ".ps1"])
def test_node_windows_implicit_interpreters_are_parent_denied(
    tmp_path: Path, suffix: str
) -> None:
    if os.name != "nt":
        pytest.skip("Windows implicit interpreter contract")
    node = _node()
    script_path = tmp_path / f"child{suffix}"
    script_path.write_text("@exit /b 91\n", encoding="utf-8")
    script = (
        "const cp=require('child_process'); let blocked=false;"
        f"try{{cp.spawnSync({json.dumps(str(script_path))},[]);}}"
        "catch(error){blocked=error.message.includes('implicit-interpreter');}"
        "if(!blocked) process.exit(17);"
    )

    completed, receipt = _run_node(
        script,
        authorities=[_authority(node), _authority(script_path)],
    )

    assert completed.returncode == 0, completed.stderr
    assert any(
        event.get("reason") == "implicit-interpreter" for event in receipt["violations"]
    )


def test_node_posix_shebang_is_parent_denied(tmp_path: Path) -> None:
    if os.name == "nt":
        pytest.skip("POSIX shebang contract")
    node = _node()
    script_path = tmp_path / "child"
    script_path.write_text("#!/usr/bin/env sh\nexit 91\n", encoding="utf-8")
    script_path.chmod(0o755)
    script = (
        "const cp=require('child_process'); let blocked=false;"
        f"try{{cp.spawnSync({json.dumps(str(script_path))},[]);}}"
        "catch(error){blocked=error.message.includes('implicit-interpreter');}"
        "if(!blocked) process.exit(17);"
    )

    completed, receipt = _run_node(
        script,
        authorities=[_authority(node), _authority(script_path)],
    )

    assert completed.returncode == 0, completed.stderr
    assert any(
        event.get("reason") == "implicit-interpreter" for event in receipt["violations"]
    )


def test_node_payload_cannot_run_when_broker_authentication_fails(
    tmp_path: Path,
) -> None:
    node = _node()
    marker = tmp_path / "payload-ran"
    script = f"require('fs').writeFileSync({json.dumps(str(marker))},'bad')"

    completed, receipt = _run_node(
        script,
        authorities=[_authority(node)],
        token_override="not-the-parent-token",
    )

    assert completed.returncode != 0
    assert not marker.exists()
    assert receipt["broker_complete"] is False
    assert any("handshake failed" in error for error in receipt["errors"])


def test_node_hook_contains_no_executable_identity_authority() -> None:
    source = (
        Path(execution_custody.__file__)
        .with_name("node_child_custody.cjs")
        .read_text(encoding="utf-8")
    )
    for forbidden in (
        "createHash",
        "existsSync",
        "readFileSync",
        "resolveExecutable",
        "policy.allowed.find",
        "moltProofCustodyTransportWorker",
    ):
        assert forbidden not in source
    assert "spawn-intent" in source
    assert "spawn-decision" in source


def test_node_hook_pins_the_current_child_policy_schema() -> None:
    """The Node hook is a second consumer of the policy contract.

    It cannot import the Python authority, so its literal is a hand-synced
    table: this gate keeps it equal to `CHILD_POLICY_SCHEMA` so a policy
    schema bump can never leave Node payloads refusing every launch.
    """
    hook = Path(execution_custody.__file__).with_name("node_child_custody.cjs")
    source = hook.read_text(encoding="utf-8")
    assert f"policy.schema !== '{execution_custody.CHILD_POLICY_SCHEMA}'" in source
    assert source.count("molt.proof-child-custody.") == 1


@pytest.mark.parametrize(
    "ending", ["natural", "process-exit", "exception", "terminate", "blocked"]
)
def test_node_worker_actual_exit_closes_its_custody(ending: str) -> None:
    worker = (
        "const {parentPort}=require('worker_threads');parentPort.postMessage('ready');"
    )
    worker += {
        "natural": "",
        "process-exit": "process.exit(7);",
        "exception": "throw new Error('worker-failure');",
        "terminate": "parentPort.on('message',()=>{});",
        "blocked": "Atomics.wait(new Int32Array(new SharedArrayBuffer(4)),0,0);",
    }[ending]
    root = (
        "const {Worker}=require('worker_threads');"
        f"const w=new Worker({json.dumps(worker)},{{eval:true}});"
        "let error=null;w.on('error',e=>{error=e.message});"
        + (
            "w.once('message',()=>w.terminate());"
            if ending in {"terminate", "blocked"}
            else ""
        )
        + "w.once('exit',code=>console.log(JSON.stringify({code,error})));"
    )
    completed, receipt = _run_node(root, authorities=[])

    assert completed.returncode == 0, completed.stderr
    outcome = json.loads(completed.stdout)
    expected_code = 7 if ending == "process-exit" else 0 if ending == "natural" else 1
    assert outcome == {
        "code": expected_code,
        "error": "worker-failure" if ending == "exception" else None,
    }
    assert execution_custody.child_receipt_is_admitted(receipt), receipt
    assert receipt["process_closure_complete"] is False
    exits = [e for e in receipt["events"] if e["event"] == "node-worker-exit"]
    assert len(exits) == 1
    assert exits[0]["exit_code"] == expected_code
    if ending in {"terminate", "blocked"}:
        assert not any(
            e["event"] == "hook-end"
            and e.get("thread_id") == exits[0]["child_thread_id"]
            for e in receipt["events"]
        )


def test_node_nested_worker_exits_are_observed_by_their_actual_creators() -> None:
    inner = (
        "const {parentPort}=require('worker_threads');parentPort.postMessage('ready');"
        "Atomics.wait(new Int32Array(new SharedArrayBuffer(4)),0,0);"
    )
    middle = (
        "const {Worker}=require('worker_threads');"
        f"const w=new Worker({json.dumps(inner)},{{eval:true,env:{{"
        "MOLT_PROOF_CHILD_CUSTODY_WORKER_PARENT:'999999',"
        "molt_proof_child_custody_worker_parent:'888888'}});"
        "w.once('message',()=>w.terminate());"
    )
    completed, receipt = _run_node(
        "const {Worker}=require('worker_threads');"
        f"new Worker({json.dumps(middle)},{{eval:true}});",
        authorities=[],
    )

    assert completed.returncode == 0, completed.stderr
    assert execution_custody.child_receipt_is_admitted(receipt), receipt
    exits = [e for e in receipt["events"] if e["event"] == "node-worker-exit"]
    assert len(exits) == 2
    assert len({e["connection_id"] for e in exits}) == 2
    assert any(e["thread_id"] != 0 for e in exits)


def test_node_immediate_worker_termination_needs_no_child_hook(record_property) -> None:
    completed, receipt = _run_node(
        "const {Worker}=require('worker_threads');"
        "const w=new Worker('',{eval:true});"
        "w.terminate().then(code=>console.log(code));",
        authorities=[],
    )

    assert completed.returncode == 0, completed.stderr
    assert execution_custody.child_receipt_is_admitted(receipt), receipt
    exits = [e for e in receipt["events"] if e["event"] == "node-worker-exit"]
    assert len(exits) == 1
    assert exits[0]["exit_code"] == int(completed.stdout)
    child_hooks = [
        e
        for e in receipt["events"]
        if e["event"] == "hook-start" and e["thread_id"] != 0
    ]
    # Startup races the caller. Retain which branch actually ran, and directly
    # prove that only creator evidence is needed when startup never reached the
    # child hook; no sleep or scheduling assumption makes this control pass.
    record_property("child_hook_starts", len(child_hooks))
    before_start = copy.deepcopy(receipt)
    child_connections = {e["connection_id"] for e in child_hooks}
    before_start["events"] = [
        e for e in before_start["events"] if e["connection_id"] not in child_connections
    ]
    assert execution_custody.child_receipt_is_admitted(before_start)


def test_node_parent_exit_does_not_complete_unobserved_workers() -> None:
    worker = (
        "const {parentPort}=require('worker_threads');parentPort.postMessage('ready');"
        "Atomics.wait(new Int32Array(new SharedArrayBuffer(4)),0,0);"
    )
    completed, receipt = _run_node(
        "const {Worker}=require('worker_threads');"
        f"const w=new Worker({json.dumps(worker)},{{eval:true}});"
        "w.once('message',()=>process.exit(0));",
        authorities=[],
    )

    assert completed.returncode == 0, completed.stderr
    assert receipt["broker_complete"] is False
    assert not execution_custody.child_receipt_is_admitted(receipt)
    assert any("creator-observed exit" in e for e in receipt["errors"])


def test_node_terminating_creator_does_not_complete_its_unobserved_child() -> None:
    inner = (
        "const {parentPort}=require('worker_threads');parentPort.postMessage('ready');"
        "Atomics.wait(new Int32Array(new SharedArrayBuffer(4)),0,0);"
    )
    middle = (
        "const {Worker,parentPort}=require('worker_threads');"
        f"const w=new Worker({json.dumps(inner)},{{eval:true}});"
        "w.once('message',()=>parentPort.postMessage('ready'));"
    )
    completed, receipt = _run_node(
        "const {Worker}=require('worker_threads');"
        f"const w=new Worker({json.dumps(middle)},{{eval:true}});"
        "w.once('message',()=>w.terminate());",
        authorities=[],
    )

    assert completed.returncode == 0, completed.stderr
    assert receipt["broker_complete"] is False
    assert any("creator-observed exit" in e for e in receipt["errors"])


def test_node_concurrent_worker_spawn_requests_preserve_each_decision() -> None:
    worker = (
        "const cp=require('child_process');"
        "for(let i=0;i<2;i++){"
        "const r=cp.spawnSync(process.execPath,['--version'],{encoding:'utf8'});"
        "if(r.status!==0||!r.stdout.startsWith('v'))throw new Error('child failed');}"
    )
    completed, receipt = _run_node(
        "const {Worker}=require('worker_threads');"
        f"for(let i=0;i<3;i++)new Worker({json.dumps(worker)},{{eval:true}});",
        authorities=[_authority(_node())],
    )

    assert completed.returncode == 0, completed.stderr
    assert execution_custody.child_receipt_is_admitted(receipt), receipt
    decisions = [e for e in receipt["events"] if e["event"] == "child-process"]
    assert len(decisions) == 6
    assert len({e["connection_id"] for e in decisions}) == 3


def test_node_terminated_worker_preserves_denied_child_request(tmp_path: Path) -> None:
    marker = tmp_path / "must-not-exist"
    child = f"require('fs').writeFileSync({json.dumps(str(marker))},'escaped')"
    worker = (
        "const {parentPort}=require('worker_threads');"
        f"try{{require('child_process').spawnSync(process.execPath,['-e',{json.dumps(child)}]);}}"
        "catch(error){parentPort.postMessage('denied');}"
        "Atomics.wait(new Int32Array(new SharedArrayBuffer(4)),0,0);"
    )
    completed, receipt = _run_node(
        "const {Worker}=require('worker_threads');"
        f"const w=new Worker({json.dumps(worker)},{{eval:true}});"
        "w.once('message',()=>w.terminate());",
        authorities=[],
    )

    assert completed.returncode == 0, completed.stderr
    assert not marker.exists()
    assert receipt["broker_complete"] is True
    assert len(receipt["violations"]) == 1
    assert not execution_custody.child_receipt_is_admitted(receipt)


def test_node_worker_exit_does_not_discard_an_unfinished_decision(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    # The broker tells the root through a pipe that the Worker's decision is
    # pending. A watched marker file made this handshake depend on
    # fseventsd, which can deliver seconds late or stall under load; the
    # Worker then never retired and the run hit its timeout.
    pending_read, pending_write = os.pipe()
    released = threading.Event()
    original = execution_custody.ChildCustodyEventServer._decide_child

    def decide(server, intent, runtime):
        if intent["requested"] == "molt-proof-release-pending-decision":
            released.set()
            return original(server, intent, runtime)
        os.write(pending_write, b"pending\n")
        # Below the run's own deadline, so this diagnostic wins over a kill.
        if not released.wait(timeout=30):
            raise RuntimeError("test parent did not observe Worker exit")
        raise RuntimeError("retained unfinished child decision")

    monkeypatch.setattr(
        execution_custody.ChildCustodyEventServer, "_decide_child", decide
    )
    worker = "require('child_process').spawnSync(process.execPath,['--version']);"
    root = (
        "const {Worker}=require('worker_threads');"
        "process.stdin.once('data',async ()=>{"
        "await w.terminate();"
        "try{require('child_process').spawnSync('molt-proof-release-pending-decision')}catch(_){}"
        "process.stdin.destroy();});"
        f"const w=new Worker({json.dumps(worker)},{{eval:true}});w.on('error',()=>{{}});"
    )
    try:
        completed, receipt = _run_node(root, authorities=[], stdin=pending_read)
    finally:
        os.close(pending_read)
        os.close(pending_write)

    assert completed.returncode == 0, completed.stderr
    assert released.is_set()
    assert any(e["event"] == "node-worker-exit" for e in receipt["events"])
    assert any("retained unfinished child decision" in e for e in receipt["errors"])
    assert receipt["broker_complete"] is False
    assert not execution_custody.child_receipt_is_admitted(receipt)


def test_node_synthetic_exit_cannot_replace_actual_worker_exit() -> None:
    worker = (
        "const {parentPort}=require('worker_threads');parentPort.postMessage('ready');"
        "Atomics.wait(new Int32Array(new SharedArrayBuffer(4)),0,0);"
    )
    completed, receipt = _run_node(
        "const {Worker}=require('worker_threads');"
        f"const w=new Worker({json.dumps(worker)},{{eval:true}});"
        "w.once('message',()=>{w.emit('exit',0);w.terminate()});",
        authorities=[],
    )

    assert completed.returncode == 0, completed.stderr
    assert receipt["broker_complete"] is True
    assert any(
        e.get("reason") == "unobserved-or-duplicate-worker-exit"
        for e in receipt["violations"]
    )
    assert not execution_custody.child_receipt_is_admitted(receipt)


def test_node_worker_receipt_rejects_forged_or_mismatched_lifetime() -> None:
    completed, receipt = _run_node(
        "const {Worker}=require('worker_threads');new Worker('',{eval:true});",
        authorities=[],
    )
    assert completed.returncode == 0, completed.stderr
    assert execution_custody.child_receipt_is_admitted(receipt), receipt
    events = receipt["events"]
    exit_index = next(
        i for i, e in enumerate(events) if e["event"] == "node-worker-exit"
    )
    start_index = next(
        i for i, e in enumerate(events) if e["event"] == "node-worker-start"
    )
    child_index = next(
        i
        for i, e in enumerate(events)
        if e["event"] == "hook-start" and e["thread_id"] != 0
    )
    for index, field, value in (
        (exit_index, "connection_id", events[child_index]["connection_id"]),
        (exit_index, "pid", events[exit_index]["pid"] + 1),
        (exit_index, "thread_id", events[child_index]["thread_id"]),
        (exit_index, "child_thread_id", events[exit_index]["child_thread_id"] + 1),
        (exit_index, "exit_code", None),
        (child_index, "pid", events[child_index]["pid"] + 1),
        (child_index, "thread_id", events[child_index]["thread_id"] + 1),
        (child_index, "parent_connection_id", events[child_index]["connection_id"]),
    ):
        forged = copy.deepcopy(receipt)
        forged["events"][index][field] = value
        assert not execution_custody.child_receipt_is_admitted(forged), (index, field)
    for index in (exit_index, start_index):
        missing = copy.deepcopy(receipt)
        del missing["events"][index]
        assert not execution_custody.child_receipt_is_admitted(missing)
        duplicate = copy.deepcopy(receipt)
        duplicate["events"].insert(index, copy.deepcopy(events[index]))
        assert not execution_custody.child_receipt_is_admitted(duplicate)
