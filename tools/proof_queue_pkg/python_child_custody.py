"""Stdlib-only child-process custody loaded before any payload imports."""

from __future__ import annotations

from collections.abc import Mapping
import json
import os
import shlex
import socket
import sys
import threading
from typing import BinaryIO

CHILD_POLICY_ENV = "MOLT_PROOF_CHILD_CUSTODY_JSON"
CHILD_ENDPOINT_ENV = "MOLT_PROOF_CHILD_CUSTODY_ENDPOINT"
CHILD_TOKEN_ENV = "MOLT_PROOF_CHILD_CUSTODY_TOKEN"


_child_channel: socket.socket | None = None
_child_channel_reader: BinaryIO | None = None
_child_channel_lock = threading.Lock()
_child_sequence = 0


def _journal(payload: Mapping[str, object]) -> None:
    if _child_channel is None:
        raise RuntimeError("proof child custody event channel is unavailable")
    line = json.dumps(dict(payload), sort_keys=True, separators=(",", ":")) + "\n"
    with _child_channel_lock:
        _child_channel.sendall(line.encode())


def _environment_value(environment: object, name: str) -> object | None:
    if not isinstance(environment, Mapping):
        return None
    expected = name.upper()
    return next(
        (
            value
            for key, value in environment.items()
            if (os.fsdecode(key) if isinstance(key, bytes) else str(key)).upper()
            == expected
        ),
        None,
    )


def _request_child_decision(
    token: object, child_env: object = None, child_cwd: object = None
) -> dict[str, object]:
    global _child_sequence
    if _child_channel is None or _child_channel_reader is None:
        raise RuntimeError("proof child custody decision channel is unavailable")
    path_value = None
    if isinstance(child_env, Mapping):
        path_value = _environment_value(child_env, "PATH")
        if isinstance(path_value, bytes):
            path_value = os.fsdecode(path_value)
    if not isinstance(path_value, str):
        path_value = os.environ.get("PATH", "")
    path_ext = None
    if isinstance(child_env, Mapping):
        path_ext = _environment_value(child_env, "PATHEXT")
        if isinstance(path_ext, bytes):
            path_ext = os.fsdecode(path_ext)
    if not isinstance(path_ext, str):
        path_ext = os.environ.get("PATHEXT", "")
    if isinstance(child_cwd, bytes):
        child_cwd = os.fsdecode(child_cwd)
    effective_cwd = (
        os.path.abspath(child_cwd)
        if isinstance(child_cwd, str) and child_cwd
        else os.getcwd()
    )
    with _child_channel_lock:
        _child_sequence += 1
        sequence = _child_sequence
        intent = {
            "event": "spawn-intent",
            "sequence": sequence,
            "requested": os.fsdecode(token) if isinstance(token, bytes) else str(token),
            "path": path_value,
            "path_ext": path_ext,
            "cwd": effective_cwd,
        }
        _child_channel.sendall(
            (json.dumps(intent, sort_keys=True, separators=(",", ":")) + "\n").encode()
        )
        raw = _child_channel_reader.readline()
    if not raw:
        raise RuntimeError("proof child custody broker closed before decision")
    decision = json.loads(raw)
    if (
        not isinstance(decision, dict)
        or decision.get("event") != "spawn-decision"
        or decision.get("sequence") != sequence
    ):
        raise RuntimeError("proof child custody broker returned an invalid decision")
    return decision


def _admit_child(
    policy: Mapping[str, object],
    token: object,
    child_env: object = None,
    child_cwd: object = None,
) -> None:
    del policy
    decision = _request_child_decision(token, child_env, child_cwd)
    if decision.get("admitted") is True:
        return
    raise PermissionError(
        f"proof child executable is outside admitted toolchain closure: {token!r}"
    )


def install_python_child_custody() -> None:
    global _child_channel, _child_channel_reader
    raw = os.environ.get(CHILD_POLICY_ENV)
    if not raw:
        return
    policy = json.loads(raw)
    if (
        not isinstance(policy, dict)
        or policy.get("schema") != "molt.proof-child-custody.v1"
    ):
        raise RuntimeError("malformed proof child custody policy")
    # Capture enforcement callables before payload execution.  The audit hook
    # must never resolve a mutable module-global name that proof code can replace
    # after bootstrap.
    admit_executable = _admit_child
    record_event = _journal
    decode_path = os.fsdecode
    split_command = shlex.split
    windows = os.name == "nt"
    endpoint = os.environ.get(CHILD_ENDPOINT_ENV, "")
    token = os.environ.get(CHILD_TOKEN_ENV, "")
    try:
        host, port_raw = endpoint.rsplit(":", 1)
        channel = socket.create_connection((host, int(port_raw)), timeout=10.0)
    except (OSError, ValueError) as exc:
        raise RuntimeError(
            f"proof child custody channel connection failed: {exc}"
        ) from exc
    channel.settimeout(None)
    _child_channel = channel
    _child_channel_reader = channel.makefile("rb")
    record_event(
        {
            "event": "hook-start",
            "runtime": "python",
            "pid": os.getpid(),
            "token": token,
            "admitted": True,
        }
    )
    ready_raw = _child_channel_reader.readline()
    if not ready_raw:
        raise RuntimeError("proof child custody broker closed before hook readiness")
    ready = json.loads(ready_raw)
    if not isinstance(ready, dict) or ready != {
        "event": "hook-ready",
        "runtime": "python",
    }:
        raise RuntimeError("proof child custody broker returned invalid hook readiness")

    import atexit

    def close_channel() -> None:
        global _child_channel, _child_channel_reader
        active = _child_channel
        if active is None:
            return
        try:
            record_event(
                {
                    "event": "hook-end",
                    "runtime": "python",
                    "pid": os.getpid(),
                    "admitted": True,
                }
            )
            active.shutdown(socket.SHUT_WR)
        finally:
            active.close()
            _child_channel = None
            _child_channel_reader = None

    atexit.register(close_channel)

    def audit(event: str, args: tuple[object, ...]) -> None:
        if event == "subprocess.Popen":
            executable = args[0] if args else None
            if executable is None and len(args) > 1:
                command_args = args[1]
                if isinstance(command_args, (list, tuple)) and command_args:
                    executable = command_args[0]
                elif isinstance(command_args, (str, bytes)):
                    command_line = (
                        decode_path(command_args)
                        if isinstance(command_args, bytes)
                        else command_args
                    )
                    split = split_command(command_line, posix=not windows)
                    executable = split[0].strip('"') if split else None
            child_env = args[3] if len(args) > 3 else None
            child_cwd = args[2] if len(args) > 2 else None
            admit_executable(policy, executable, child_env, child_cwd)
        elif event in {
            "os.system",
            "os.exec",
            "os.posix_spawn",
            "os.posix_spawnp",
            "os.spawn",
            "os.fork",
            "os.forkpty",
        }:
            record_event({"event": "policy-violation", "surface": event})
            raise PermissionError(
                f"opaque process creation is forbidden in proof custody: {event}"
            )

    sys.addaudithook(audit)
    # The bootstrap loaded this authority by a private file-module name.  Remove
    # every alias to that module before returning to payload code so the proof
    # cannot mutate enforcement globals through sys.modules.
    authority_module = sys.modules.get(__name__)
    if authority_module is not None:
        for module_name, loaded in tuple(sys.modules.items()):
            if loaded is authority_module:
                sys.modules.pop(module_name, None)
