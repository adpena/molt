from __future__ import annotations

import json
import subprocess
import sys
from pathlib import Path

import pytest

from molt import python_interpreter
from molt import toolchain_identity
from tests import molt_diff
from tests.process_guard_common import install_module_view


@pytest.mark.parametrize("mode", ["current", "uv", "path-candidates"])
def test_verifier_resolution_fences_every_early_candidate(tmp_path, monkeypatch, mode):
    prefix = tmp_path / "verifier"
    prefix.mkdir()
    target = python_interpreter.parse_target_python_version(
        f"{sys.version_info.major}.{sys.version_info.minor}"
    )
    calls = []
    monkeypatch.setattr(
        toolchain_identity,
        "find_executable",
        lambda *_args, **_kwargs: Path("uv") if mode == "uv" else None,
    )
    monkeypatch.setattr(
        python_interpreter,
        "target_python_command_candidates",
        lambda *_args, **_kwargs: [["first-candidate"], ["second-candidate"]],
    )

    def run(command, **_kwargs):
        calls.append(command)
        if command[0] == "uv":
            assert command == ["uv", "python", "find", target.short]
            return "selected-python\n", "", 0
        # Assert before returning any candidate identity, including failed ones.
        assert command[1:7] == [
            "-B",
            "-I",
            "-X",
            f"pycache_prefix={prefix}",
            "-S",
            "-c",
        ]
        if command[0] == "first-candidate":
            return "", "independent unavailable candidate", 127
        return (
            json.dumps(
                {
                    "implementation": "CPython",
                    "version": target.short + ".1",
                    "executable": str(tmp_path / "selected-python"),
                }
            ),
            "",
            0,
        )

    monkeypatch.setattr(python_interpreter, "_run_command", run)
    selected = python_interpreter.resolve_target_python_command(
        target,
        prefer_current=mode == "current",
        prefer_uv=mode == "uv",
        verifier_bytecode_prefix=prefix,
    )
    expected = {
        "current": (sys.executable,),
        "uv": ("selected-python",),
        "path-candidates": ("second-candidate",),
    }[mode]
    assert selected == expected
    assert len(calls) == (1 if mode == "current" else 2)


@pytest.mark.parametrize(
    "selector",
    ["3.14", "path", "py -3.14", "py -V:Vendor/3.14", "py -3.14 -X dev"],
)
def test_verifier_selector_preserves_command_transport(tmp_path, monkeypatch, selector):
    prefix = tmp_path / "verifier"
    prefix.mkdir()
    executable = tmp_path / "selected Python.exe"
    executable.write_bytes(b"never executed")
    if selector == "path":
        selector = str(executable)
    calls = []
    monkeypatch.setattr(toolchain_identity, "find_executable", lambda *_a, **_kw: None)

    def run(command, **_kwargs):
        calls.append(command)
        assert command[-7:-1] == [
            "-B",
            "-I",
            "-X",
            f"pycache_prefix={prefix}",
            "-S",
            "-c",
        ]
        return (
            json.dumps(
                {
                    "implementation": "CPython",
                    "version": "3.14.1",
                    "executable": str(executable),
                }
            ),
            "",
            0,
        )

    monkeypatch.setattr(python_interpreter, "_run_command", run)
    command = python_interpreter.resolve_python_selector(
        selector, verifier_bytecode_prefix=prefix
    )
    selected = python_interpreter.probe_python_command(
        command, verifier_bytecode_prefix=prefix
    )
    assert selected.command == command
    assert calls[-1][:-7] == list(command)
    if selector.startswith("py "):
        assert command == tuple(selector.split())
    elif selector == str(executable):
        assert command == (str(executable),)


@pytest.mark.parametrize(
    "options",
    [
        ("-c", "raise SystemExit(0)"),
        ("-Imjson",),
        ("script.py",),
        ("--",),
        ("-X",),
        ("-X", "pycache_prefix=foreign"),
        ("-IXpycache_prefix=foreign",),
    ],
)
def test_verifier_probe_refuses_prior_payload_or_cache_policy(
    tmp_path, monkeypatch, options
):
    prefix = tmp_path / "verifier"
    prefix.mkdir()

    def forbidden(*_args, **_kwargs):
        raise AssertionError("rejected command must not execute")

    monkeypatch.setattr(python_interpreter, "_run_command", forbidden)
    with pytest.raises(
        python_interpreter.PythonInterpreterError, match="verifier Python"
    ):
        python_interpreter.probe_python_command(
            ("py", "-3.14", *options), verifier_bytecode_prefix=prefix
        )


@pytest.mark.parametrize(
    ("stdout", "stderr", "expected"),
    [
        ("text out", "text err", ("text out", "text err", 124)),
        (b"byte out", b"byte err", ("byte out", "byte err", 124)),
        (None, None, ("", "timeout", 124)),
    ],
)
def test_timeout_payloads_are_normalized_at_interpreter_boundary(
    stdout: str | bytes | None,
    stderr: str | bytes | None,
    expected: tuple[str, str, int],
    monkeypatch: pytest.MonkeyPatch,
    tmp_path: Path,
) -> None:
    def time_out(*_args, **_kwargs):
        raise subprocess.TimeoutExpired(
            ["python"],
            1.0,
            output=stdout,
            stderr=stderr,
        )

    install_module_view(
        monkeypatch, "subprocess", subprocess, python_interpreter, run=time_out
    )

    assert (
        python_interpreter._run_command(
            ["python"],
            timeout=1.0,
            env={},
            cwd=tmp_path,
        )
        == expected
    )


def test_target_python_candidates_are_cross_platform_without_ambient_default():
    target = python_interpreter.parse_target_python_version("3.14")

    candidates = python_interpreter.target_python_command_candidates(target)

    assert [sys.executable] not in candidates
    assert ["python3.14"] in candidates
    assert ["python314"] in candidates
    if sys.platform.startswith("win"):
        assert ["py", "-3.14"] in candidates


def test_target_python_override_preserves_multi_token_command():
    target = python_interpreter.parse_target_python_version("3.13")

    assert python_interpreter.target_python_command_candidates(
        target,
        override="py -3.13",
    ) == [["py", "-3.13"]]


def test_selector_resolves_version_through_verified_command_authority(monkeypatch):
    calls: list[str] = []

    def fake_resolve(target, **kwargs):
        calls.append(target.short)
        assert kwargs["prefer_current"] is True
        return ("py", f"-{target.short}")

    monkeypatch.setattr(
        python_interpreter, "resolve_target_python_command", fake_resolve
    )

    assert python_interpreter.resolve_python_selector("3.14") == ("py", "-3.14")
    assert calls == ["3.14"]


def test_selector_preserves_explicit_path_and_command(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
):
    executable = tmp_path / "python custom.exe"
    executable.write_text("", encoding="utf-8")

    assert python_interpreter.resolve_python_selector(str(executable)) == (
        str(executable),
    )
    assert python_interpreter.resolve_python_selector("py -3.13") == (
        "py",
        "-3.13",
    )
    assert python_interpreter.resolve_python_selector(f'"{executable}" -X dev') == (
        str(executable),
        "-X",
        "dev",
    )
    monkeypatch.chdir(tmp_path)
    relative = (
        ".\\python custom.exe"
        if sys.platform.startswith("win")
        else "./python custom.exe"
    )
    assert python_interpreter.resolve_python_selector(f'"{relative}"') == (relative,)


def test_selector_refuses_missing_explicit_path(tmp_path: Path):
    missing = tmp_path / "missing" / "python.exe"
    with pytest.raises(
        python_interpreter.PythonInterpreterError,
        match="Python interpreter not found",
    ):
        python_interpreter.resolve_python_selector(str(missing))


def test_target_python_resolution_fails_closed_with_attempts(monkeypatch):
    target = python_interpreter.parse_target_python_version("3.13")
    monkeypatch.setattr(
        toolchain_identity, "find_executable", lambda _name, **_kwargs: None
    )

    def fake_run_command(command, **_kwargs):
        return "", f"missing {' '.join(command)}", 127

    monkeypatch.setattr(python_interpreter, "_run_command", fake_run_command)

    with pytest.raises(RuntimeError) as excinfo:
        python_interpreter.resolve_target_python_command(target)

    message = str(excinfo.value)
    assert "no verified CPython 3.13 command available" in message
    if sys.platform.startswith("win"):
        assert "py -3.13" in message
    assert "python3.13" in message


def test_molt_diff_python_version_uses_shared_multi_token_resolver(monkeypatch):
    calls: list[tuple[str, object]] = []

    def fake_resolve(target_python, **kwargs):
        calls.append((target_python.short, kwargs.get("cwd")))
        return ["py", "-3.13"]

    monkeypatch.setattr(
        molt_diff.python_interpreter,
        "resolve_target_python_command",
        fake_resolve,
    )

    assert molt_diff._resolve_diff_python_command("3.13") == ("py", "-3.13")
    assert calls == [("3.13", molt_diff._repo_root())]


def test_molt_diff_metadata_probe_preserves_multi_token_python_command(monkeypatch):
    commands: list[list[str]] = []
    molt_diff._METADATA_PROBE_CACHE.clear()

    def fake_probe(command, *, budget=None):
        # The probe's protocol: one JSON line naming the executable and the
        # feature version (molt_diff._python_command_metadata).
        commands.append(command)
        return subprocess.CompletedProcess(
            command,
            0,
            '{"executable": "C:\\\\Python313\\\\python.exe", "version_info": [3, 13]}\n',
            "",
        )

    monkeypatch.setattr(molt_diff, "_run_metadata_probe", fake_probe)

    assert molt_diff._python_exe_version(("py", "-3.13")) == (3, 13)
    (command,) = commands
    assert command[:3] == ["py", "-3.13", "-c"] and len(command) == 4


def test_python_explicit_command_uses_host_quoted_option_authority(tmp_path):
    executable = tmp_path / "selected Python.exe"
    executable.write_bytes(b"selected interpreter image")
    assert python_interpreter.explicit_python_command(
        f'"{executable}" -X pycache_prefix="selected cache"'
    ) == (str(executable), "-X", "pycache_prefix=selected cache")


def test_python_explicit_command_preserves_parser_failure_contract(monkeypatch):
    from molt import toolchain_identity

    def invalid(_value):
        raise ValueError("native parser refused command")

    monkeypatch.setattr(toolchain_identity, "split_native_command", invalid)
    with pytest.raises(
        python_interpreter.PythonInterpreterError, match="native parser refused command"
    ):
        python_interpreter.explicit_python_command("selected-python --invalid")


def test_interpreter_candidates_share_callers_remaining_probe_budget(monkeypatch):
    remaining = [2.0]
    caps = []
    exhausted = RuntimeError("caller budget exhausted")

    def limit(cap):
        if remaining[0] == 0:
            raise exhausted
        return min(cap, remaining[0])

    def run(command, *, timeout, env, cwd):
        caps.append(timeout)
        remaining[0] -= 1
        return "", "missing", 127

    monkeypatch.setattr(python_interpreter, "_run_command", run)
    monkeypatch.setattr(
        toolchain_identity, "find_executable", lambda name, **_kwargs: Path("uv")
    )
    with pytest.raises(RuntimeError) as error:
        python_interpreter.resolve_target_python_command(
            python_interpreter.parse_target_python_version("3.13"), probe_timeout=limit
        )
    assert error.value is exhausted
    assert caps == [2, 1]
