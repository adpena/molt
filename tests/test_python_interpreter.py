from __future__ import annotations

import json
import subprocess
import sys
from pathlib import Path

import pytest

from molt import python_interpreter
from molt import toolchain_identity
from tests import molt_diff
from molt.python_environment_identity import PythonIdentityVerifier


@pytest.mark.parametrize("mode", ["current", "uv", "path-candidates"])
def test_verifier_resolution_fences_every_early_candidate(
    verifier, tmp_path, monkeypatch, mode
):
    prefix = verifier.prefix
    request = python_interpreter.parse_python_interpreter_request(
        ".".join(map(str, sys.version_info[:3]))
    )
    from molt import process_guard
    from tests.process_guard_common import guard_result_for_test

    calls = []
    monkeypatch.setattr(
        toolchain_identity,
        "find_executable",
        lambda name, **_kwargs: (
            (Path("uv") if mode == "uv" else None) if name == "uv" else Path(name)
        ),
    )
    monkeypatch.setattr(
        python_interpreter,
        "python_command_candidates",
        lambda *_args, **_kwargs: [["first-candidate"], ["second-candidate"]],
    )

    def run(command, **kwargs):
        assert kwargs["memory_guard_prefix"] == "MOLT_PYTHON_IDENTITY"
        calls.append(command)
        if command[0] == "uv":
            assert command == ["uv", "python", "find", request.selector]
            return guard_result_for_test(
                command, 0, "selected-python\n", "", descendants_closed=True
            )
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
            return guard_result_for_test(
                command,
                127,
                "",
                "independent unavailable candidate",
                descendants_closed=True,
            )
        return guard_result_for_test(
            command,
            0,
            json.dumps(
                {
                    "implementation": "CPython",
                    "version": request.selector,
                    "executable": str(tmp_path / "selected-python"),
                }
            ),
            "",
            descendants_closed=True,
        )

    monkeypatch.setattr(process_guard, "run_completed_command", run)
    selected = python_interpreter.resolve_python_interpreter(
        request,
        prefer_current=mode == "current",
        prefer_uv=mode == "uv",
        verifier=verifier,
    )
    expected = {
        "current": (sys.executable,),
        "uv": ("selected-python",),
        "path-candidates": ("second-candidate",),
    }[mode]
    assert selected.command == expected
    assert len(calls) == (1 if mode == "current" else 2)


@pytest.mark.parametrize(
    "selector",
    ["3.14", "path", "py -3.14", "py -V:Vendor/3.14", "py -3.14 -X dev"],
)
def test_verifier_selector_preserves_command_transport(
    verifier, tmp_path, monkeypatch, selector
):
    prefix = verifier.prefix
    executable = tmp_path / "selected Python.exe"
    executable.write_bytes(b"never executed")
    if selector == "path":
        selector = str(executable)
    calls = []
    monkeypatch.setattr(
        toolchain_identity,
        "find_executable",
        lambda name, **_kw: None if name == "uv" else Path(name),
    )

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
    command = python_interpreter.resolve_python_selector(selector, verifier=verifier)
    selected = python_interpreter.probe_python_command(command, verifier=verifier)
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
    verifier, tmp_path, monkeypatch, options
):
    def forbidden(*_args, **_kwargs):
        raise AssertionError("rejected command must not execute")

    monkeypatch.setattr(python_interpreter, "_run_command", forbidden)
    with pytest.raises(
        python_interpreter.PythonInterpreterError, match="verifier Python"
    ):
        python_interpreter.probe_python_command(
            ("py", "-3.14", *options), verifier=verifier
        )


@pytest.mark.parametrize("mode", ["script", "module"])
def test_readonly_reference_startup_preserves_site_environment_and_payload(
    tmp_path, mode
):
    import os

    from molt.python_environment_identity import PythonIdentityVerifier

    project = tmp_path / "project"
    project.mkdir()
    module = project / "reference_subject.py"
    module.write_text(
        "import json, os, sys\n"
        "import reference_dependency\n"
        "print(json.dumps({\n"
        " 'prefix': sys.pycache_prefix, 'no_write': sys.dont_write_bytecode,\n"
        " 'isolated': sys.flags.isolated, 'ignore_environment': sys.flags.ignore_environment,\n"
        " 'site': 'site' in sys.modules, 'argv': sys.argv[1:],\n"
        " 'environment': os.environ['REFERENCE_ENV'],\n"
        " 'dependency': reference_dependency.VALUE,\n"
        "}))\n",
        encoding="utf-8",
    )
    dependency = project / "reference_dependency.py"
    dependency.write_text("VALUE = 73\n", encoding="utf-8")
    foreign = tmp_path / "foreign-cache"
    foreign.mkdir()
    sentinel = foreign / "retained"
    sentinel.write_bytes(b"independent foreign namespace must remain unchanged")
    env = dict(os.environ)
    env.update(
        {
            "PYTHONPATH": str(project),
            "PYTHONPYCACHEPREFIX": str(foreign),
            "PYTHONDONTWRITEBYTECODE": "0",
            "REFERENCE_ENV": "retained payload environment",
        }
    )
    with PythonIdentityVerifier() as verifier:
        root = verifier.prefix
        command = python_interpreter.readonly_python_command(
            (sys.executable,), environment_root=root
        )
        payload = [str(module)] if mode == "script" else ["-m", "reference_subject"]
        result = verifier.run(
            [*command, *payload, "--payload-flag", "-Xpayload-only"],
            cwd=project,
            env=env,
            capture_output=True,
            text=True,
            check=True,
            memory_guard_prefix="MOLT_PYTEST",
            encoding="utf-8",
        )
        assert result.stderr == ""
        observed = json.loads(result.stdout)
        assert observed == {
            "prefix": str(root / "bytecode"),
            "no_write": True,
            "isolated": 0,
            "ignore_environment": 0,
            "site": True,
            "argv": ["--payload-flag", "-Xpayload-only"],
            "environment": "retained payload environment",
            "dependency": 73,
        }
        assert not list(root.iterdir())
    assert not root.exists()
    assert not (project / "__pycache__").exists()
    assert list(foreign.iterdir()) == [sentinel]
    assert (
        sentinel.read_bytes() == b"independent foreign namespace must remain unchanged"
    )


@pytest.mark.parametrize(
    "options",
    [
        ("-c", "pass"),
        ("-Imjson",),
        ("script.py",),
        ("--",),
        ("-X",),
        ("-X", "pycache_prefix=foreign"),
        ("-IXpycache_prefix=foreign",),
    ],
)
def test_readonly_command_refuses_prior_payload_or_cache_policy(tmp_path, options):
    with pytest.raises(
        python_interpreter.PythonInterpreterError, match="read-only Python"
    ):
        python_interpreter.readonly_python_command(
            ("py", "-3.14", *options), environment_root=tmp_path
        )


def test_readonly_command_preserves_launcher_and_explicit_startup_semantics(tmp_path):
    assert python_interpreter.readonly_python_command(
        ("py", "-V:Vendor/3.14", "-I", "-S", "-X", "dev"),
        environment_root=tmp_path,
    ) == (
        "py",
        "-V:Vendor/3.14",
        "-I",
        "-S",
        "-X",
        "dev",
        "-B",
        "-X",
        f"pycache_prefix={tmp_path / 'bytecode'}",
    )


@pytest.mark.parametrize("closed", [False, True])
def test_timeout_preserves_original_guard_receipt_at_interpreter_boundary(
    monkeypatch, tmp_path, closed
):
    from types import SimpleNamespace
    from molt import process_guard
    from molt.python_environment_identity import PythonIdentityVerifier

    receipt = SimpleNamespace(descendants_closed=closed)
    original = subprocess.TimeoutExpired(
        ["python"], 1.0, output=b"raw out", stderr=b"raw err"
    )
    original.guarded_result = receipt

    def time_out(command, **kwargs):
        assert kwargs["memory_guard_prefix"] == "MOLT_PYTHON_IDENTITY"
        raise original

    monkeypatch.setattr(process_guard, "run_completed_command", time_out)
    with pytest.raises(subprocess.TimeoutExpired) as caught:
        with PythonIdentityVerifier() as owner:
            path = owner.prefix
            python_interpreter._run_command(
                ["python"],
                verifier=owner,
                timeout=1.0,
                env={},
                cwd=tmp_path,
            )
    assert caught.value is original
    assert caught.value.guarded_result is receipt
    assert caught.value.output == b"raw out" and caught.value.stderr == b"raw err"
    assert path.exists() is (not closed)
    if not closed:
        # No real child exists at this synthetic guard seam; preserve the live
        # allocation's original inode authority for explicit test retirement.
        owner._allocation.cleanup()


def test_target_python_candidates_are_cross_platform_without_ambient_default():
    target = python_interpreter.parse_target_python_version("3.14")

    candidates = python_interpreter.python_command_candidates(
        python_interpreter.PythonInterpreterRequest(target.major, target.minor)
    )

    assert [sys.executable] not in candidates
    assert ["python3.14"] in candidates
    assert ["python314"] in candidates
    if sys.platform.startswith("win"):
        assert ["py", "-3.14"] in candidates


def test_target_python_override_preserves_multi_token_command():
    target = python_interpreter.parse_target_python_version("3.13")

    assert python_interpreter.python_command_candidates(
        python_interpreter.PythonInterpreterRequest(target.major, target.minor),
        override="py -3.13",
    ) == [["py", "-3.13"]]


def test_selector_resolves_version_through_verified_command_authority(
    verifier, monkeypatch
):
    calls: list[str] = []

    def fake_resolve(request, **kwargs):
        calls.append(request.selector)
        assert kwargs["prefer_current"] is True
        return python_interpreter.PythonInterpreter(
            ("py", "-3.15"), "selected-python", "3.15.1", "CPython"
        )

    monkeypatch.setattr(python_interpreter, "resolve_python_interpreter", fake_resolve)

    assert python_interpreter.resolve_python_selector("3.15.1", verifier=verifier) == (
        "py",
        "-3.15",
    )
    assert calls == ["3.15.1"]


@pytest.mark.parametrize(
    "value", ["3", "py315", "03.15", "3.015", "3.15.1.0", "3.11", "3.15t"]
)
def test_host_request_refuses_noncanonical_or_unsupported_selection(value):
    with pytest.raises(ValueError):
        python_interpreter.parse_python_interpreter_request(value)


@pytest.mark.parametrize(
    "selector,version_shaped",
    [
        ("3.15.1", True),
        ("3.15", True),
        ("3", True),
        ("py315", True),
        ("py -3.15", False),
        ("/owned/python3.15", False),
        ("python3.15 -I", False),
    ],
)
def test_physical_selector_shape_does_not_turn_commands_into_guest_targets(
    selector, version_shaped
):
    assert python_interpreter.is_python_version_selector(selector) is version_shaped


def test_host_request_preserves_patch_without_admitting_guest_semantics():
    request = python_interpreter.parse_python_interpreter_request("3.15.1")
    assert request.selector == "3.15.1"
    assert request.matches_version("3.15.1")
    assert not request.matches_version("3.15.0")
    assert not request.matches_version("3.15.10")
    assert not request.matches_version("3.15.1rc1")
    assert not request.matches_version("3.15.1.0")
    assert not request.matches_version("3.15.1+foreign")
    assert not request.matches_version("3.15.1.post1")
    assert python_interpreter.parse_python_interpreter_request("3.15").matches_version(
        "3.15.10"
    )
    with pytest.raises(ValueError, match="unsupported Python target"):
        python_interpreter.parse_target_python_version("3.15")


@pytest.mark.parametrize("reported", ["3.15.0", "3.15.10", "3.15.1rc1", "3.15.1.0"])
def test_explicit_host_patch_mismatch_refuses_after_only_identity_probe(
    verifier, tmp_path, monkeypatch, reported
):
    executable = tmp_path / "selected-python"
    executable.write_bytes(b"never execute this fixture")
    prefix = verifier.prefix
    calls = []

    def run(command, **kwargs):
        calls.append(command)
        assert command[:7] == [
            str(executable),
            "-B",
            "-I",
            "-X",
            f"pycache_prefix={prefix}",
            "-S",
            "-c",
        ]
        assert len(command) == 8
        return (
            json.dumps(
                {
                    "implementation": "CPython",
                    "version": reported,
                    "executable": str(executable),
                }
            ),
            "",
            0,
        )

    monkeypatch.setattr(python_interpreter, "_run_command", run)
    with pytest.raises(
        python_interpreter.PythonInterpreterError,
        match="no verified CPython 3.15.1",
    ):
        python_interpreter.resolve_python_interpreter(
            python_interpreter.parse_python_interpreter_request("3.15.1"),
            override=str(executable),
            verifier=verifier,
        )
    assert len(calls) == 1


def test_guest_oracle_cannot_admit_host_only_minor(verifier, monkeypatch):
    monkeypatch.setattr(
        python_interpreter,
        "resolve_python_interpreter",
        lambda *_a, **_k: pytest.fail("unsupported guest reached physical discovery"),
    )
    with pytest.raises(ValueError, match="outside the supported target set"):
        python_interpreter.resolve_target_python(
            python_interpreter.TargetPythonVersion(3, 15, 1), verifier=verifier
        )

    monkeypatch.setattr(
        python_interpreter,
        "probe_python_command",
        lambda *_a, **_k: pytest.fail("unsupported guest reached metadata execution"),
    )
    accepted, detail = python_interpreter.verify_target_python_command(
        ("host-only-python",),
        target_python=python_interpreter.TargetPythonVersion(3, 15, 1),
        verifier=verifier,
    )
    assert not accepted and "outside the supported target set" in detail


def test_minimum_host_request_does_not_use_guest_catalog(verifier, monkeypatch):
    monkeypatch.setattr(
        python_interpreter.platform, "python_implementation", lambda: "PyPy"
    )
    calls = []

    def resolve(request, **kwargs):
        calls.append(request.selector)
        return python_interpreter.PythonInterpreter(
            ("host-python",), "host-python", "3.15.1", "CPython"
        )

    monkeypatch.setattr(python_interpreter, "resolve_python_interpreter", resolve)
    assert python_interpreter.python_command_for_min_version(
        (3, 15), verifier=verifier
    ) == ("host-python",)
    assert calls == ["3.15"]


def test_selector_preserves_explicit_path_and_command(
    verifier, tmp_path: Path, monkeypatch: pytest.MonkeyPatch
):
    executable = tmp_path / "python custom.exe"
    executable.write_text("", encoding="utf-8")

    assert python_interpreter.resolve_python_selector(
        str(executable), verifier=verifier
    ) == (str(executable),)
    assert python_interpreter.resolve_python_selector(
        "py -3.13", verifier=verifier
    ) == (
        "py",
        "-3.13",
    )
    assert python_interpreter.resolve_python_selector(
        f'"{executable}" -X dev', verifier=verifier
    ) == (
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
    assert python_interpreter.resolve_python_selector(
        f'"{relative}"', verifier=verifier
    ) == (relative,)


def test_selector_refuses_missing_explicit_path(verifier, tmp_path: Path):
    missing = tmp_path / "missing" / "python.exe"
    with pytest.raises(
        python_interpreter.PythonInterpreterError,
        match="Python interpreter not found",
    ):
        python_interpreter.resolve_python_selector(str(missing), verifier=verifier)


def test_target_python_resolution_fails_closed_with_attempts(verifier, monkeypatch):
    target = python_interpreter.parse_target_python_version("3.13")
    monkeypatch.setattr(
        toolchain_identity, "find_executable", lambda _name, **_kwargs: None
    )

    def fake_run_command(command, **_kwargs):
        pytest.fail(f"unavailable candidate reached a child launch: {command}")

    monkeypatch.setattr(python_interpreter, "_run_command", fake_run_command)

    with pytest.raises(RuntimeError) as excinfo:
        python_interpreter.resolve_target_python_command(target, verifier=verifier)

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
    commands: list[tuple[str, ...]] = []
    molt_diff._METADATA_PROBE_CACHE.clear()

    def fake_probe(command, *, budget=None):
        commands.append(command)
        return python_interpreter.PythonInterpreter(
            tuple(command), r"C:\Python313\python.exe", "3.13.17", "CPython"
        )

    monkeypatch.setattr(molt_diff, "_run_metadata_probe", fake_probe)

    assert molt_diff._python_exe_version(("py", "-3.13")) == (3, 13)
    (command,) = commands
    assert command == ("py", "-3.13")


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


def test_interpreter_candidates_share_callers_remaining_probe_budget(
    verifier, monkeypatch
):
    remaining = [2.0]
    caps = []
    exhausted = RuntimeError("caller budget exhausted")

    def limit(cap):
        if remaining[0] == 0:
            raise exhausted
        return min(cap, remaining[0])

    def run(command, *, verifier, timeout, env, cwd):
        caps.append(timeout)
        remaining[0] -= 1
        return "", "missing", 127

    monkeypatch.setattr(python_interpreter, "_run_command", run)
    monkeypatch.setattr(
        toolchain_identity, "find_executable", lambda name, **_kwargs: Path("uv")
    )
    with pytest.raises(RuntimeError) as error:
        python_interpreter.resolve_target_python_command(
            python_interpreter.parse_target_python_version("3.13"),
            probe_timeout=limit,
            verifier=verifier,
        )
    assert error.value is exhausted
    assert caps == [2, 1]


@pytest.fixture
def verifier():
    with PythonIdentityVerifier() as owner:
        yield owner
