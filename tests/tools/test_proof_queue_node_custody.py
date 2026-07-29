from __future__ import annotations

import hashlib
import json
import os
import shutil
import subprocess
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
) -> tuple[subprocess.CompletedProcess[str], dict[str, object]]:
    node = _node()
    policy = {
        "schema": "molt.proof-child-custody.v1",
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
            check=False,
            capture_output=True,
            text=True,
            timeout=30,
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
        event
        for event in receipt["events"]
        if event.get("event") == "child-process"
    ]
    assert len(child_events) == 1
    assert child_events[0]["admitted"] is True
    assert Path(str(child_events[0]["resolved"])).resolve() == node
    assert child_events[0]["toolchain"] == "node"


def test_node_process_exit_closes_broker_terminal_handshake() -> None:
    completed, receipt = _run_node(
        "process.exit(0)", authorities=[_authority(_node())]
    )

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
    assert sum(
        event.get("event") == "child-process" for event in receipt["events"]
    ) == 1


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
    starts = [event for event in receipt["events"] if event.get("event") == "hook-start"]
    ends = [event for event in receipt["events"] if event.get("event") == "hook-end"]
    assert len(starts) == len(ends) == 3
    assert any(
        event.get("reason") == "opaque-shell" for event in receipt["violations"]
    )
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
    starts = [event for event in receipt["events"] if event.get("event") == "hook-start"]
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
    assert any(
        event.get("reason") == "opaque-shell" for event in receipt["violations"]
    )


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
        event.get("reason") == "implicit-interpreter"
        for event in receipt["violations"]
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
        event.get("reason") == "implicit-interpreter"
        for event in receipt["violations"]
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
