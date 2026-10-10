"""Stdlib-only child-process custody loaded before any payload imports."""

from __future__ import annotations

from collections.abc import Callable, Mapping
import json
import os
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


def _posix_launch(
    token: object, child_env: object, child_cwd: object
) -> dict[str, object]:
    """Report what CPython's POSIX launch searches: the child's PATH and cwd."""
    # Match the actual CPython child launch: an explicit environment without
    # PATH uses os.defpath, while None inherits and an empty PATH names cwd.
    # The stdlib also owns bytes-environment and POSIX key-case semantics.
    path_value = os.pathsep.join(
        os.get_exec_path(child_env if isinstance(child_env, Mapping) else None)
    )
    if isinstance(child_cwd, bytes):
        child_cwd = os.fsdecode(child_cwd)
    effective_cwd = (
        os.path.abspath(child_cwd)
        if isinstance(child_cwd, str) and child_cwd
        else os.getcwd()
    )
    return {
        "requested": os.fsdecode(token) if isinstance(token, bytes) else str(token),
        "path": path_value,
        "cwd": effective_cwd,
    }


def _windows_caller_facts() -> Callable[[], dict[str, object]]:
    """Bind the calling-process facts that CreateProcessW searches with.

    Windows resolves a launch in the calling process, so the child's env and
    cwd take no part. PATH comes from the process environment block, which
    os.putenv changes without updating os.environ.
    """
    import _winapi
    import ctypes
    from ctypes import wintypes

    module_file_name = _winapi.GetModuleFileName
    need_current_directory = _winapi.NeedCurrentDirectoryForExePath
    current_directory = os.getcwd
    get_variable = ctypes.WinDLL(
        "kernel32", use_last_error=True
    ).GetEnvironmentVariableW
    get_variable.argtypes = [wintypes.LPCWSTR, wintypes.LPWSTR, wintypes.DWORD]
    get_variable.restype = wintypes.DWORD
    create_buffer = ctypes.create_unicode_buffer
    get_last_error = ctypes.get_last_error
    set_last_error = ctypes.set_last_error
    error_envvar_not_found = 203

    def process_path() -> str | None:
        size = 1024
        while True:
            buffer = create_buffer(size)
            set_last_error(0)
            length = get_variable("PATH", buffer, size)
            if length == 0:
                error = get_last_error()
                if error == error_envvar_not_found:
                    return None
                if error:
                    raise OSError(error, "cannot read the process PATH")
                return ""
            if length < size:
                return buffer.value
            size = length

    def caller_facts() -> dict[str, object]:
        return {
            "caller_image": module_file_name(0),
            "caller_cwd": current_directory(),
            "caller_path": process_path(),
            # A name without a backslash follows NoDefaultCurrentDirectoryInExePath.
            "caller_searches_current_directory": bool(need_current_directory("molt")),
        }

    return caller_facts


def _request_child_decision(launch: Mapping[str, object]) -> dict[str, object]:
    global _child_sequence
    if _child_channel is None or _child_channel_reader is None:
        raise RuntimeError("proof child custody decision channel is unavailable")
    with _child_channel_lock:
        _child_sequence += 1
        sequence = _child_sequence
        intent = {"event": "spawn-intent", "sequence": sequence, **launch}
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


def _admit_child(launch: Mapping[str, object]) -> None:
    decision = _request_child_decision(launch)
    if decision.get("admitted") is True:
        return
    requested = launch.get("requested") or launch.get("application_name")
    raise PermissionError(
        "proof child executable is outside admitted toolchain closure: "
        f"{requested or launch.get('command_line')!r}"
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
    posix_launch = _posix_launch
    windows = os.name == "nt"
    caller_facts = _windows_caller_facts() if windows else None
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
            # CPython raises this event as (executable, args, cwd, env) just
            # before it launches. On Windows it passes executable as
            # lpApplicationName and args, already a list2cmdline string, as
            # lpCommandLine; CreateProcessW then searches with this process's
            # facts. POSIX searches the child's PATH from the child's cwd.
            if caller_facts is not None:
                launch = {
                    "application_name": args[0] if args else None,
                    "command_line": args[1] if len(args) > 1 else None,
                    **caller_facts(),
                }
            else:
                launch = posix_launch(
                    args[0] if args else None,
                    args[3] if len(args) > 3 else None,
                    args[2] if len(args) > 2 else None,
                )
            admit_executable(launch)
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
