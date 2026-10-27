"""Exec wrappers preserve the inner admission authority for every family."""

from __future__ import annotations

import os

import pytest

from tools import proof_command_argv, proof_plan, venv_exec
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
        ["python", "-m", "json.tool"],
        ["python", "tools/check_format.py"],
        ["python", "-c", "print(1)"],
        ["python", "-"],
        ["node", "unregistered.js"],
    ],
)
def test_unrelated_wrapped_payload_is_admitted_without_wrapper_row(wrapper, payload):
    command = [*wrapper, "--venv", ".venv", "--", *payload]
    root = command_admission._REPO_ROOT
    assert proof_command_argv._command_entrypoint(
        command, root=root
    ) == proof_command_argv._command_entrypoint(payload, root=root)
    envelope = command_admission.envelope_for_command(command)
    assert envelope["kind"] != "rejected"
    assert envelope["argv"] == payload
    assert envelope["proof_plan_command_ids"] == []


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
    assert proof_command_argv._command_entrypoint(
        command, root=command_admission._REPO_ROOT
    ) == (
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


@pytest.mark.parametrize(
    "wrapper",
    [
        ["python", "tools/venv_exec.py", "--venv", ".venv"],
        ["python", "tools/uv_project_env.py", "--python", "3.12"],
    ],
)
def test_native_wrapper_declares_its_image_without_relabeling_payload(wrapper):
    import copy

    command = [*wrapper, "--", "git", "status", "--short"]
    envelope = command_admission.envelope_for_command(command)
    assert envelope["argv"] == ["git", "status", "--short"]
    assert envelope["python"] is None
    assert set(envelope["toolchains"]) == {"git", "python"}
    assert set(envelope["process_closure"]["toolchains"]) == {"git", "python"}
    command_admission.validate_envelope(envelope, command)
    incomplete = copy.deepcopy(envelope)
    incomplete["toolchains"].remove("python")
    incomplete["process_closure"]["toolchains"].remove("python")
    with pytest.raises(ValueError, match="does not match submitted argv"):
        command_admission.validate_envelope(incomplete, command)


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

    def capture_context(admitted, **kwargs):
        seen.append(admitted)
        return kwargs["env"]

    monkeypatch.setattr(
        execution_environment,
        "_require_cargo_build_tool_environment_context",
        capture_context,
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
    from molt.cargo_execution_policy import default_nested_process_timeout_seconds
    from tools.proof_queue_pkg import command_identity
    from tests.process_guard_common import run_custody_subject_process

    # A nested real pytest under the custody bootstrap takes 9-17 s on an idle
    # host and more under load, so it shares the one nested execution budget
    # instead of a fixed wall clock (review v3, finding 9).
    deadline = default_nested_process_timeout_seconds("execution")
    events = []
    errors = []
    listener = socket.socket()
    listener.bind(("127.0.0.1", 0))
    listener.listen(1)
    listener.settimeout(deadline)

    def broker():
        try:
            with listener, listener.accept()[0] as client:
                client.settimeout(deadline)
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
        {
            "python": {
                "executable": exact[0],
                "base_executable": sys._base_executable,
                "location": {"prefix": sys.prefix},
            }
        },
        environment=environment,
        cwd=tmp_path,
    )
    env = {
        **environment,
        **updates,
        "MOLT_PROOF_CHILD_CUSTODY_JSON": json.dumps(
            {"schema": "molt.proof-child-custody.v2"}
        ),
        "MOLT_PROOF_CHILD_CUSTODY_ENDPOINT": endpoint,
        "MOLT_PROOF_CHILD_CUSTODY_TOKEN": "test-only-token",
        "PYTEST_DISABLE_PLUGIN_AUTOLOAD": "1",
    }
    completed = run_custody_subject_process(
        launch,
        cwd=tmp_path,
        env=env,
        timeout=deadline,
        capture_output=True,
        text=True,
        check=False,
    )
    thread.join(timeout=deadline)
    assert not thread.is_alive(), "custody broker did not terminate"
    assert not errors, errors
    output = tmp_path / "stdout.bin"
    output.write_text(completed.stdout, encoding="utf-8")
    transcript = command_identity._transcript_identity(output)
    return completed, events, launch, transcript


def test_wrapped_pytest_runs_selected_venv_under_real_audit_and_requires_counts(
    tmp_path,
    monkeypatch,
):
    import copy
    import json
    import os
    import sys
    from pathlib import Path
    from tools.proof_queue_pkg import command_identity, execution_environment

    venv = Path(sys.executable).parent.parent
    scratch = tmp_path / "proof-scratch"
    payload = tmp_path / "test_payload.py"
    payload.write_text(
        f"import sys\nfrom pathlib import Path\ndef test_selected_runtime(cache):\n    assert Path(sys.prefix).resolve() == Path({str(venv)!r}).resolve()\n    cache.set('custody/wrapper', {{'retained': True}})\n",
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
        "--strict-config",
        "-W",
        "error",
        "-c",
        str(Path(__file__).resolve().parents[2] / "pyproject.toml"),
        str(payload),
    ]
    # An ad hoc pytest run is admitted only for owned repository tests; this
    # payload lives outside the checkout, so only this fixture's exact row
    # admits it, and an altered command is still refused.
    with pytest.raises(ValueError, match="outside the checkout"):
        command_admission.envelope_for_command(submitted)
    registry = copy.deepcopy(command_admission._proof_command_registry())
    registry["exact"][tuple(submitted)] = {
        "ids": ["fixture.wrapper-pytest"],
        "toolchains": ("python",),
        "cargo_native_units": {},
    }
    monkeypatch.setattr(command_admission, "_proof_command_registry", lambda: registry)
    envelope = command_admission.envelope_for_command(submitted)
    assert envelope["kind"] == "proof-plan"
    assert envelope["proof_plan_command_ids"] == ["fixture.wrapper-pytest"]
    with pytest.raises(ValueError, match="outside the checkout"):
        command_admission.envelope_for_command([*submitted, "-x"])
    assert envelope["submitted_argv"] == submitted
    assert command_admission.command_proof_kind(envelope) == "test-execution"
    command_admission.validate_envelope(envelope, submitted)
    environment = execution_environment._wrapper_execution_environment(
        envelope, {**os.environ, "MOLT_PROOF_SCRATCH_ROOT": str(scratch)}
    )
    exact = command_identity._exact_command(envelope, cwd=tmp_path, env=environment)
    assert Path(exact[0]).resolve() == venv_exec.venv_python(venv).resolve()
    completed, events, launch, transcript = _run_under_real_child_audit(
        envelope, exact, environment, tmp_path
    )
    assert completed.returncode == 0, completed.stdout + completed.stderr
    assert "no:cacheprovider" not in launch
    assert json.loads(
        (scratch / "pytest-cache" / "v/custody/wrapper").read_text(encoding="utf-8")
    ) == {"retained": True}
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
    spelling, tmp_path, monkeypatch
):
    from functools import partial
    import os
    from tools import uv_project_env
    from tools.proof_queue_pkg import execution_environment
    from tests.python_environment_test_support import (
        materialize_development_environment,
    )

    root, artifacts = tmp_path / "project", tmp_path / "artifacts"
    image = materialize_development_environment(root, artifacts, python="3.14")
    explicit = image.root
    # Keep parsing and pure selection real; operation capture owns recipe
    # admission. This test explicitly selects managed ownership.
    monkeypatch.delenv("MOLT_VENV", raising=False)
    monkeypatch.setattr(
        uv_project_env,
        "uv_project_env",
        partial(uv_project_env.uv_project_env, repo_root=root),
    )
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
            "MOLT_EXT_ROOT": str(artifacts),
        },
    )
    assert env["UV_PROJECT_ENVIRONMENT"] == str(explicit.resolve())
    assert env["MOLT_UV_PROJECT_PYTHON"] == "3.14"
    assert env["UV_NO_SYNC"] == "1"
    assert env["PYTHONPYCACHEPREFIX"] == str(image.bytecode_prefix)
    assert env["PYTHONDONTWRITEBYTECODE"] == "1"
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
        "forward_command",
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


def test_registry_projected_beside_a_redirected_root_keeps_plan_entrypoints(
    tmp_path, monkeypatch
):
    """HF-368: a redirected admission root cannot poison later admission.

    A fixture that pointed the admission root at an empty tree made the first
    registry projection of its pytest worker there. Wrapped plan commands then
    registered under the wrapper's own entrypoint, and every later
    ``venv_exec.py --help`` query in that worker failed as a near-match.
    """
    wrapped = next(
        command
        for command in proof_plan.ProofPlan.load().commands
        if command.argv[1:2] == ("tools/venv_exec.py",)
        and not command.binds_source_commit
    )
    # Make the redirected projection the first one in this process.
    command_admission._proof_command_registry.cache_clear()
    try:
        with monkeypatch.context() as redirected:
            redirected.setattr(command_admission, "_REPO_ROOT", tmp_path)
            command_admission._proof_command_registry()
        help_query = command_admission.envelope_for_command(
            ["python", "tools/venv_exec.py", "--help"]
        )
        assert help_query["argv"] == ["python", "tools/venv_exec.py", "--help"]
        assert "wrapper" not in help_query
        admitted = command_admission.envelope_for_command(list(wrapped.argv))
        assert admitted["kind"] == "proof-plan"
        assert wrapped.id in admitted["proof_plan_command_ids"]
    finally:
        command_admission._proof_command_registry.cache_clear()


@pytest.mark.parametrize(
    "spelling", ["tools/./venv_exec.py", "tools/../tools/venv_exec.py"]
)
def test_repository_wrapper_component_aliases_keep_inner_authority(spelling):
    envelope = command_admission.envelope_for_command(
        ["python", spelling, "--", "python", "-m", "json.tool"]
    )
    assert envelope["argv"] == ["python", "-m", "json.tool"]
    assert envelope["proof_plan_command_ids"] == []
    # The wrapped pytest run meets the same admission as the bare one: no
    # single proof-plan command owns the whole tests tree.
    for command in (
        ["python", spelling, "--", "python", "-m", "pytest", "tests"],
        ["python", "-m", "pytest", "tests"],
    ):
        with pytest.raises(ValueError, match="tests has no owning proof-plan command"):
            command_admission.envelope_for_command(command)


@pytest.mark.parametrize("seed", [None, "0", "123", "random"])
@pytest.mark.parametrize("flags", [[], ["-E"], ["-I"], ["-R"]])
def test_wrapper_preserves_first_payload_cpython_hash_policy(tmp_path, seed, flags):
    import json
    import os
    import sys
    from pathlib import Path
    from molt.python_environment_location import environment_bytecode_arguments
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
    # The independent reference must retain the runner's image policy even
    # when the hash-policy subject deliberately ignores environment variables.
    reference_command = [
        exact[0],
        *environment_bytecode_arguments(Path(sys.prefix)),
        *exact[1:],
    ]
    reference = run_custody_subject_process(
        reference_command,
        cwd=tmp_path,
        env=selected,
        capture_output=True,
        text=True,
        timeout=30,
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


@pytest.mark.parametrize(
    "ownership", ["uv-project", "managed-venv", "argument", "environment"]
)
def test_native_wrapper_capture_selects_published_entrypoint_or_explicit_borrow(
    tmp_path, ownership
):
    import os
    from molt.python_environment_location import environment_python_path
    from tools.proof_queue_pkg import execution_environment

    root = tmp_path / "selected"
    canonical = environment_python_path(root)
    canonical.parent.mkdir(parents=True)
    canonical.write_bytes(b"canonical interpreter")
    alias = canonical.parent / ("python3.exe" if os.name == "nt" else "python3")
    alias.write_bytes(b"different alias interpreter")
    wrapper = (
        ["python", "tools/uv_project_env.py", "--python", "3.12"]
        if ownership == "uv-project"
        else ["python", "tools/venv_exec.py"]
    )
    if ownership == "argument":
        wrapper.extend(["--venv", str(root)])
    submitted = [*wrapper, "--", "git", "status", "--short"]
    envelope = command_admission.envelope_for_command(submitted)
    payload = ["git", "status", "--short"]
    env = {"VIRTUAL_ENV": str(root)}
    if ownership == "environment":
        env["MOLT_VENV"] = str(root)

    dependency, command = execution_environment._python_capture_envelope(
        envelope, payload, env
    )

    borrowed = ownership in {"argument", "environment"}
    expected = alias if borrowed and os.name != "nt" else canonical
    assert command[0] == str(expected)
    assert dependency["wrapper"] == envelope["wrapper"]
    assert envelope["argv"] == payload
    assert envelope["python"] is None
    # A missing published entrypoint cannot turn an available alias into the
    # image's identity. The locator must receive the missing canonical path and
    # fail there; only explicit borrowing retains the generic alias search.
    if not borrowed:
        canonical.unlink()
        _, missing = execution_environment._python_capture_envelope(
            envelope, payload, env
        )
        assert missing[0] == str(canonical)
        assert not canonical.exists() and alias.is_file()


@pytest.mark.parametrize("borrowed", [False, True])
def test_modeled_venv_python_matches_direct_wrapper_selection_and_startup(
    tmp_path, borrowed
):
    import os
    from molt.python_environment_location import environment_python_path
    from tools.proof_queue_pkg import command_identity

    root = tmp_path / "selected"
    canonical = environment_python_path(root)
    canonical.parent.mkdir(parents=True)
    canonical.write_bytes(b"canonical interpreter")
    canonical.chmod(0o755)
    alias = canonical.parent / ("python3.exe" if os.name == "nt" else "python3")
    alias.write_bytes(b"different alias interpreter")
    alias.chmod(0o755)
    submitted = ["python", "tools/venv_exec.py"]
    if borrowed:
        submitted.extend(["--venv", str(root)])
    payload = ["python", "-I", "-c", "pass"]
    submitted.extend(["--", *payload])
    envelope = command_admission.envelope_for_command(submitted)
    exact = command_identity._exact_command(
        envelope, cwd=tmp_path, env={"VIRTUAL_ENV": str(root)}
    )

    expected = alias if borrowed and os.name != "nt" else canonical
    startup = [] if borrowed else ["-B", "-X", f"pycache_prefix={root / 'bytecode'}"]
    assert exact == [str(expected), *startup, *payload[1:]]
    assert exact == venv_exec.resolve_command(payload, venv=root, managed=not borrowed)


@pytest.mark.parametrize("explicit_handoff", [False, True])
def test_failed_exec_restores_retained_shared_source_descriptor(
    tmp_path, explicit_handoff
):
    import os
    import sys
    from pathlib import Path
    from tests.process_guard_common import run_custody_subject_process

    if os.name != "posix":
        pytest.skip("POSIX exec descriptor transfer")
    code = """
import os, sys
from pathlib import Path
from molt.file_locks import _try_acquire_file_lock, _release_file_lock, _file_lock_is_owned
from molt.process_spawn import forward_command
root = Path(sys.argv[1])
lock = root / 'source.lock'
handle = _try_acquire_file_lock(lock, shared=True)
assert handle is not None
before = os.get_inheritable(handle.file.fileno())
try:
    try:
        forward_command([str(root / 'missing-program')], env=os.environ,
                        retained_shared_handles=(handle,),
                        handoff_argument="--state-handoff" if sys.argv[2] == "True" else None)
    except FileNotFoundError:
        pass
    else:
        raise AssertionError('missing executable was accepted')
    assert os.get_inheritable(handle.file.fileno()) == before
    assert _file_lock_is_owned(handle)
    assert _try_acquire_file_lock(lock) is None
finally:
    _release_file_lock(handle)
exclusive = _try_acquire_file_lock(lock)
assert exclusive is not None
_release_file_lock(exclusive)
"""
    result = run_custody_subject_process(
        [sys.executable, "-B", "-c", code, str(tmp_path), str(explicit_handoff)],
        cwd=Path(__file__).resolve().parents[2],
        env=dict(os.environ),
        capture_output=True,
        text=True,
        timeout=30,
    )
    assert result.returncode == 0, result.stderr


def test_no_site_detached_ack_retains_selected_image_without_activation(tmp_path):
    import os
    from pathlib import Path
    from tests.process_guard_common import run_custody_subject_process
    from tests.python_environment_test_support import (
        materialize_development_environment,
    )
    from molt.checkout_state import record_checkout_owner
    from molt.python_environment_location import environment_python_path

    owner = tmp_path / "source-owner"
    image = materialize_development_environment(
        owner, tmp_path / "artifacts", runnable=True
    )
    record_checkout_owner(image.root, owner=owner, kind="python-environment")
    root = Path(__file__).resolve().parents[2]
    code = f"""
import sys
from pathlib import Path
sys.path[:0] = [{str(root / "src")!r}, {str(root)!r}]
from tools.command_execution import detached_command_use
from molt.checkout_state import checkout_state_retirement
image = Path(sys.argv[1])
ready = Path(sys.argv[2])
assert sys.flags.no_site
# CPython 3.12 actually exposes the base prefix with -S. Preserve that
# adversarial coordinate on newer CPython too, without changing its executable.
sys.prefix = sys.base_prefix
assert Path(sys.prefix) != image
assert Path(sys.executable).parent.parent == image
with checkout_state_retirement(image) as available:
    assert available
with detached_command_use(Path.cwd(), {{}}, cwd=Path.cwd(),
                          ready_path=ready, use_token='a' * 32):
    assert ready.exists()
    with checkout_state_retirement(image) as available:
        assert not available, 'acknowledged worker lost its selected image'
with checkout_state_retirement(image) as available:
    assert available
"""
    result = run_custody_subject_process(
        [
            str(environment_python_path(image.root)),
            "-I",
            "-S",
            "-B",
            "-c",
            code,
            str(image.root),
            str(tmp_path / "ready.json"),
        ],
        cwd=root,
        env=dict(os.environ),
        capture_output=True,
        text=True,
        timeout=30,
    )
    assert result.returncode == 0, result.stderr


@pytest.mark.skipif(os.name != "posix", reason="POSIX inherited descriptor admission")
@pytest.mark.parametrize("defect", ["pid", "inode", "unknown", "noninheritable", "own"])
def test_exec_handoff_rejects_substitution_before_changing_any_descriptor(
    tmp_path, defect
):
    import dataclasses
    from molt.file_locks import (
        _file_lock_descriptor_action,
        _try_acquire_file_lock,
        _release_file_lock,
    )
    from molt.process_spawn import ExecSharedDescriptor, ExecStateHandoff

    lock = tmp_path / "source.lock"
    source = _try_acquire_file_lock(lock, shared=True)
    assert source is not None
    incoming = []
    try:
        with _file_lock_descriptor_action():
            for _ in range(2):
                fd = os.dup(source.file.fileno())
                os.set_inheritable(fd, True)
                incoming.append(fd)
        metadata = lock.stat()
        items = [
            ExecSharedDescriptor(fd, lock, metadata.st_dev, metadata.st_ino)
            for fd in incoming
        ]
        pid = os.getpid()
        if defect == "pid":
            pid += 1
        elif defect == "inode":
            items[1] = dataclasses.replace(items[1], inode=metadata.st_ino + 1)
        elif defect == "unknown":
            other = tmp_path / "unrelated-data"
            other.write_text("not a state lock", encoding="utf-8")
            with _file_lock_descriptor_action():
                os.close(incoming[1])
                incoming[1] = os.open(other, os.O_RDONLY)
                os.set_inheritable(incoming[1], True)
            other_stat = other.stat()
            items[1] = ExecSharedDescriptor(
                incoming[1], other, other_stat.st_dev, other_stat.st_ino
            )
        elif defect == "noninheritable":
            with _file_lock_descriptor_action():
                os.set_inheritable(incoming[1], False)
        elif defect == "own":
            items[1] = dataclasses.replace(items[1], fd=source.file.fileno())
        packet = ExecStateHandoff.from_json(
            ExecStateHandoff(pid, tuple(items)).to_json()
        )
        with pytest.raises(ValueError):
            packet.receive(source)
        assert os.get_inheritable(incoming[0]), (
            "partial admission changed an earlier descriptor"
        )
        assert os.fstat(incoming[0]).st_ino == metadata.st_ino
        assert _try_acquire_file_lock(lock) is None
    finally:
        with _file_lock_descriptor_action():
            for fd in incoming:
                os.close(fd)
        _release_file_lock(source)


@pytest.mark.parametrize(
    "defect",
    ["boolean_pid", "stdio", "duplicate", "extra", "relative", "duplicate_key"],
)
def test_exec_handoff_wire_rejects_ambiguous_descriptors(tmp_path, defect):
    import json
    from molt.process_spawn import ExecStateHandoff

    row = {"fd": 5, "path": str(tmp_path / "source.lock"), "device": 1, "inode": 2}
    payload = {"schema": "molt.exec-state-handoff.v1", "pid": 42, "descriptors": [row]}
    if defect == "boolean_pid":
        payload["pid"] = True
    elif defect == "stdio":
        row["fd"] = 1
    elif defect == "duplicate":
        payload["descriptors"].append(dict(row))
    elif defect == "extra":
        payload["roots"] = []
    elif defect == "relative":
        row["path"] = "source.lock"
    raw = json.dumps(payload)
    if defect == "duplicate_key":
        raw = '{"pid":1,' + raw[1:]
    with pytest.raises(ValueError):
        ExecStateHandoff.from_json(raw)
