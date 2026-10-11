"""Exec wrappers preserve the inner admission authority for every family."""

from __future__ import annotations

import pytest

from tools import proof_plan, venv_exec
from tools.proof_queue_pkg import command_admission


@pytest.mark.parametrize(
    "wrapper",
    [
        ["python", "tools/venv_exec.py"],
        ["python", "-m", "tools.venv_exec"],
        [
            "uv",
            "run",
            "--active",
            "--project",
            ".",
            "--python",
            "3.12",
            "--no-sync",
            "--no-config",
            "--offline",
            "python",
            "-I",
            "tools/venv_exec.py",
        ],
        ["py", "-3.12", "-B", "tools/venv_exec.py"],
    ],
)
@pytest.mark.parametrize(
    "payload",
    [
        ["python", "-m", "ruff", "check", "."],
        ["python", "-m", "pytest", "tests/test_dummy.py"],
        ["python", "tools/check_format.py"],
        ["python", "-c", "print(1)"],
        ["python", "-"],
        ["node", "unregistered.js"],
    ],
)
def test_unrelated_wrapped_payload_is_admitted_without_wrapper_row(wrapper, payload):
    command = [*wrapper, "--venv", ".venv", "--", *payload]
    assert command_admission._command_entrypoint(
        command
    ) == command_admission._command_entrypoint(payload)
    envelope = command_admission.envelope_for_command(command)
    assert envelope["kind"] != "rejected"


@pytest.mark.parametrize(
    "target",
    [
        ["-m", "molt"],
        ["-m", "molt.cli"],
        ["-m", "molt.__main__"],
        ["-m", "molt.cli.__main__"],
        ["-m", "molt.cli.entrypoint"],
        ["src/molt/__main__.py"],
        ["src/molt/cli/__main__.py"],
        ["src/molt/cli/entrypoint.py"],
    ],
)
@pytest.mark.parametrize(
    "wrapper",
    [[], ["python", "tools/venv_exec.py"], ["python", "-m", "tools.venv_exec"]],
)
def test_cli_aliases_cannot_discard_registered_build_authority(target, wrapper):
    command = [*wrapper, "python", "-B", *target, "build", "unregistered.py"]
    assert command_admission._command_entrypoint(command) == (
        "python-cli-command",
        "molt.cli:build",
    )
    with pytest.raises(ValueError, match="near-match.*toolchain authority"):
        command_admission.envelope_for_command(command)


@pytest.mark.parametrize("option", ["--ve", "--ven", "--v"])
def test_wrapper_abbreviations_rejected_by_execution_and_admission(option):
    with pytest.raises(SystemExit) as caught:
        venv_exec.main([option, ".venv", "python", "-c", "pass"])
    assert caught.value.code == 2
    with pytest.raises(ValueError, match="invalid venv wrapper options"):
        command_admission.envelope_for_command(
            ["python", "tools/venv_exec.py", option, ".venv", "python", "-c", "pass"]
        )


def test_exact_registered_wrapper_preserves_full_toolchain_authority():
    row = next(
        row
        for row in proof_plan.ProofPlan.load().commands
        if row.id == "wasm.test.control-flow"
    )
    envelope = command_admission.envelope_for_command(row.argv)
    assert envelope["proof_plan_command_ids"] == [row.id]
    assert set(row.toolchains) <= set(envelope["toolchains"])


def test_nested_wrappers_are_rejected_without_recursive_failure():
    command = ["python", *["tools/venv_exec.py", "python"] * 1000, "-c", "pass"]
    envelope = command_admission.admission_envelope(command)
    assert envelope["kind"] == "rejected"
    assert "one typed layer" in envelope["error"]


def test_declared_python_cargo_driver_keeps_complete_output_policy(monkeypatch):
    from pathlib import Path
    from tools.proof_queue_pkg import execution_environment, cargo_output_environment

    row = next(
        row
        for row in proof_plan.ProofPlan.load().commands
        if row.id == "wasm.test.control-flow"
    )
    envelope = command_admission.envelope_for_command(row.argv)
    assert "cargo" in envelope["toolchains"]
    assert command_admission.cargo_invocation_for_envelope(envelope) is None
    assert cargo_output_environment.CargoOutputEnvironment.for_envelope(
        envelope
    ).documentation
    seen = []
    monkeypatch.setattr(
        execution_environment,
        "_require_cargo_build_tool_environment_context",
        lambda admitted, **kwargs: seen.append(admitted),
    )
    monkeypatch.setattr(
        execution_environment.toolchain_capture,
        "select_cargo_build_tool_environment",
        lambda **kwargs: ({"RUSTDOC": "bound-rustdoc"}, {}),
    )
    selected, contract = execution_environment._bind_cargo_build_tool_environment(
        envelope, {}, {"override_names": []}, cwd=Path.cwd()
    )
    assert seen == [envelope]
    assert selected["RUSTDOC"] == "bound-rustdoc"
    assert "RUSTDOC" in contract["passed_names"]


def _run_under_real_child_audit(envelope, exact, environment, tmp_path):
    import json
    import socket
    import sys
    import threading
    from tools.proof_queue_pkg import command_identity
    from tests.process_guard_common import run_custody_subject_process

    events = []
    errors = []
    listener = socket.socket()
    listener.bind(("127.0.0.1", 0))
    listener.listen(1)
    listener.settimeout(30)

    def broker():
        try:
            with listener, listener.accept()[0] as client:
                client.settimeout(30)
                with client.makefile("rb") as stream:
                    while line := stream.readline():
                        event = json.loads(line)
                        events.append(event)
                        if event["event"] == "hook-start":
                            client.sendall(
                                b'{"event":"hook-ready","runtime":"python"}\n'
                            )
                        elif event["event"] == "spawn-intent":
                            client.sendall(
                                (
                                    json.dumps(
                                        {
                                            "event": "spawn-decision",
                                            "sequence": event["sequence"],
                                            "admitted": False,
                                        }
                                    )
                                    + "\n"
                                ).encode()
                            )
        except Exception as exc:
            errors.append(exc)

    thread = threading.Thread(target=broker, daemon=True)
    endpoint = f"127.0.0.1:{listener.getsockname()[1]}"
    thread.start()
    launch, updates = command_admission._supervised_execution_command(
        envelope,
        exact,
        {"python": {"executable": exact[0], "base_executable": sys._base_executable}},
    )
    env = {
        **environment,
        **updates,
        "MOLT_PROOF_CHILD_CUSTODY_JSON": json.dumps(
            {"schema": "molt.proof-child-custody.v1"}
        ),
        "MOLT_PROOF_CHILD_CUSTODY_ENDPOINT": endpoint,
        "MOLT_PROOF_CHILD_CUSTODY_TOKEN": "test-only-token",
        "PYTEST_DISABLE_PLUGIN_AUTOLOAD": "1",
    }
    completed = run_custody_subject_process(
        launch,
        cwd=tmp_path,
        env=env,
        timeout=30,
        capture_output=True,
        text=True,
        check=False,
    )
    thread.join(timeout=30)
    assert not thread.is_alive(), "custody broker did not terminate"
    assert not errors, errors
    output = tmp_path / "stdout.bin"
    output.write_text(completed.stdout, encoding="utf-8")
    transcript = command_identity._transcript_identity(output)
    return completed, events, launch, transcript


def test_wrapped_pytest_runs_selected_venv_under_real_audit_and_requires_counts(
    tmp_path,
):
    import os
    import sys
    from pathlib import Path
    from tools.proof_queue_pkg import command_identity, execution_environment

    venv = Path(sys.executable).parent.parent
    payload = tmp_path / "test_payload.py"
    payload.write_text(
        f"import sys\nfrom pathlib import Path\ndef test_selected_runtime():\n    assert Path(sys.prefix).resolve() == Path({str(venv)!r}).resolve()\n",
        encoding="utf-8",
    )
    submitted = [
        sys.executable,
        "tools/venv_exec.py",
        "--venv",
        str(venv),
        "--",
        "python",
        "-m",
        "pytest",
        "-q",
        str(payload),
    ]
    envelope = command_admission.envelope_for_command(submitted)
    assert envelope["submitted_argv"] == submitted
    assert command_admission.command_proof_kind(envelope) == "test-execution"
    command_admission.validate_envelope(envelope, submitted)
    environment = execution_environment._wrapper_execution_environment(
        envelope, os.environ
    )
    exact = command_identity._exact_command(envelope, cwd=tmp_path, env=environment)
    assert Path(exact[0]).resolve() == venv_exec.venv_python(venv).resolve()
    completed, events, launch, transcript = _run_under_real_child_audit(
        envelope, exact, environment, tmp_path
    )
    assert completed.returncode == 0, completed.stderr
    assert "no:cacheprovider" in launch
    assert transcript["test_counts"] == {"passed": 1}
    assert not any(event["event"] == "policy-violation" for event in events)
    assert any(event["event"] == "hook-start" for event in events)
    assert any(event["event"] == "hook-end" for event in events)
    command_identity.validate_structured_test_counts(
        envelope, {"stdout": transcript}, returncode=0
    )
    with pytest.raises(ValueError, match="structured test-count"):
        command_identity.validate_structured_test_counts(envelope, {}, returncode=0)


def test_os_exec_remains_forbidden_by_actual_child_audit(tmp_path):
    import os
    import sys

    command = [
        sys.executable,
        "-c",
        "import os,sys\ntry: os.execvpe(sys.executable,[sys.executable,'-c','pass'],os.environ)\nexcept PermissionError: print('blocked')\nelse: raise AssertionError('exec escaped custody')",
    ]
    envelope = command_admission.envelope_for_command(command)
    completed, events, _, _ = _run_under_real_child_audit(
        envelope, command, os.environ, tmp_path
    )
    assert completed.returncode == 0, completed.stderr
    assert completed.stdout.strip() == "blocked"
    assert any(
        event.get("surface") == "os.exec" and event["event"] == "policy-violation"
        for event in events
    )


def test_wrapped_cargo_cannot_bypass_queue_policy():
    from tools.proof_queue_pkg import policy

    error = policy._proof_command_policy_error(
        ["python", "tools/venv_exec.py", "--", "cargo", "build"]
    )
    assert error and "raw `cargo`" in error


def test_wrapped_node_keeps_node_payload_custody():
    envelope = command_admission.envelope_for_command(
        ["python", "tools/venv_exec.py", "--", "node", "script.js"]
    )
    assert envelope["argv"] == ["node", "script.js"]
    assert envelope["python"] is None
    assert "node" in envelope["toolchains"]


@pytest.mark.parametrize(
    "spelling", [["tools/uv_project_env.py"], ["-m", "tools.uv_project_env"]]
)
def test_uv_project_wrapper_binds_declared_policy_without_claiming_runtime(
    spelling, tmp_path
):
    import os
    from tools.proof_queue_pkg import execution_environment

    explicit = tmp_path / "uv-environment"
    command = [
        "python",
        *spelling,
        "--python",
        "3.14",
        "--purpose",
        "wrapper-test",
        "--venv",
        str(explicit),
        "--",
        "python",
        "-c",
        "pass",
    ]
    envelope = command_admission.envelope_for_command(command)
    assert envelope["argv"] == ["python", "-c", "pass"]
    env = execution_environment._wrapper_execution_environment(
        envelope,
        {
            **os.environ,
            "MOLT_EXT_ROOT": str(tmp_path / "artifacts"),
        },
    )
    assert env["UV_PROJECT_ENVIRONMENT"] == str(explicit.resolve())
    assert env["MOLT_UV_PROJECT_PYTHON"] == "3.14"
    assert envelope["python"]["kind"] == "direct"
    assert (
        "version" not in envelope["python"]
    )  # The runtime capture owns observed version.


@pytest.mark.parametrize("option", ["--py", "--pur", "--ve"])
def test_uv_project_abbreviations_are_rejected_without_any_exec(option, monkeypatch):
    from tools import uv_project_env

    monkeypatch.setattr(
        uv_project_env,
        "uv_project_env",
        lambda *args, **kwargs: pytest.fail(
            "invalid options reached environment creation"
        ),
    )
    monkeypatch.setattr(
        uv_project_env,
        "run_command",
        lambda *args, **kwargs: pytest.fail("invalid options reached execution"),
    )
    with pytest.raises(SystemExit) as caught:
        uv_project_env.main([option, "3.12", "python", "-c", "pass"])
    assert caught.value.code == 2


def test_nonpython_venv_payload_does_not_invent_an_interpreter_requirement(tmp_path):
    from tools.proof_queue_pkg import execution_environment

    absent = tmp_path / "no-python-needed"
    command = [
        "python",
        "tools/venv_exec.py",
        "--venv",
        str(absent),
        "--",
        "node",
        "script.js",
    ]
    envelope = command_admission.envelope_for_command(command)
    env = execution_environment._wrapper_execution_environment(envelope, {})
    assert env["VIRTUAL_ENV"] == str(absent.resolve())
    assert envelope["python"] is None


@pytest.mark.parametrize("script", ["tools/venv_exec.py", "tools/uv_project_env.py"])
def test_wrapper_help_remains_a_terminal_leaf_query(script):
    envelope = command_admission.envelope_for_command(["python", script, "--help"])
    assert envelope["argv"] == ["python", script, "--help"]
    assert "wrapper" not in envelope


@pytest.mark.parametrize(
    "spelling", ["tools/./venv_exec.py", "tools/../tools/venv_exec.py"]
)
def test_repository_wrapper_component_aliases_keep_inner_authority(spelling):
    envelope = command_admission.envelope_for_command(
        ["python", spelling, "--", "python", "-m", "pytest", "tests"]
    )
    assert envelope["argv"] == ["python", "-m", "pytest", "tests"]
    assert command_admission.command_proof_kind(envelope) == "test-execution"


@pytest.mark.parametrize("seed", [None, "0", "123", "random"])
@pytest.mark.parametrize("flags", [[], ["-E"], ["-I"], ["-R"]])
def test_wrapper_preserves_first_payload_cpython_hash_policy(tmp_path, seed, flags):
    import json
    import os
    import sys
    from pathlib import Path
    from tests.process_guard_common import run_custody_subject_process
    from tools.proof_queue_pkg import command_identity, execution_environment

    payload = "import json,os,sys;print(json.dumps([hash('molt-seed'),hash(b'molt-seed'),os.environ.get('PYTHONHASHSEED'),sys.flags.hash_randomization,sys.flags.ignore_environment,sys.flags.isolated]))"
    environment = dict(os.environ)
    if seed is None:
        environment.pop("PYTHONHASHSEED", None)
    else:
        environment["PYTHONHASHSEED"] = seed
    command = [
        sys.executable,
        "tools/venv_exec.py",
        "--venv",
        str(Path(sys.prefix)),
        "--",
        "python",
        *flags,
        "-c",
        payload,
    ]
    envelope = command_admission.envelope_for_command(command)
    selected = execution_environment._wrapper_execution_environment(
        envelope, environment
    )
    assert selected.get("PYTHONHASHSEED") == seed
    exact = command_identity._exact_command(envelope, cwd=tmp_path, env=selected)
    reference = run_custody_subject_process(
        exact, cwd=tmp_path, env=selected, capture_output=True, text=True, timeout=30
    )
    completed, events, _, _ = _run_under_real_child_audit(
        envelope, exact, selected, tmp_path
    )
    assert reference.returncode == completed.returncode == 0, (
        reference.stderr,
        completed.stderr,
    )
    direct = json.loads(reference.stdout)
    wrapped = json.loads(completed.stdout)
    assert wrapped[2:] == direct[2:]
    assert wrapped[2] == seed
    if not flags and seed in {"0", "123"}:
        assert wrapped == direct  # independently started same-interpreter literal seed
    else:
        assert wrapped[3] == 1  # random starts: no probabilistic unequal-hash oracle
    assert [event["event"] for event in events] == ["hook-start", "hook-end"]


@pytest.mark.parametrize("seed", [None, "123"])
def test_actual_molt_help_has_one_custodied_interpreter_without_restart(tmp_path, seed):
    import os
    import sys
    from pathlib import Path
    from tools.proof_queue_pkg import command_identity, execution_environment

    environment = dict(os.environ)
    if seed is None:
        environment.pop("PYTHONHASHSEED", None)
    else:
        environment["PYTHONHASHSEED"] = seed
    environment["PYTHONPATH"] = str(Path(__file__).resolve().parents[2] / "src")
    submitted = [
        sys.executable,
        "tools/venv_exec.py",
        "--venv",
        str(Path(sys.prefix)),
        "--",
        "python",
        "-m",
        "molt.cli",
        "--help",
    ]
    envelope = command_admission.envelope_for_command(submitted)
    selected = execution_environment._wrapper_execution_environment(
        envelope, environment
    )
    assert selected.get("PYTHONHASHSEED") == seed
    exact = command_identity._exact_command(envelope, cwd=tmp_path, env=selected)
    completed, events, _, _ = _run_under_real_child_audit(
        envelope, exact, selected, tmp_path
    )
    assert completed.returncode == 0, completed.stderr
    assert "usage:" in completed.stdout
    assert [event["event"] for event in events] == ["hook-start", "hook-end"]
