"""Cross-backend divergence sub-oracle tests (doc 66 FACT 2).

The single-backend differential (native-only) makes a backend-specific
divergence INVISIBLE: if wasm/llvm/luau produces a different answer than
native/CPython, no gate goes red. doc 66's multi-backend oracle closes that by
(a) comparing every requested backend against CPython under the ONE comparison
law and (b) comparing the backends against EACH OTHER. A FAIL means any backend
disagrees with CPython OR any two backends disagree with each other.

These tests prove the MECHANISM at unit speed (no real compiler build) by:
  * exercising `molt_diff._cross_backend_divergence` directly, and
  * driving the real `molt_diff.diff_test` multi-backend path with an in-memory
    fake backend registry, so a synthetic per-backend wrong answer is witnessed
    as a FAIL — the unforgeable proof that a backend fork cannot pass silently.

The heavy end-to-end proof (real native + wasm builds + a fault-injected wrong
answer) is run separately via tests/molt_diff.py --target; this file is the fast,
deterministic regression that the divergence logic itself is correct.
"""

from __future__ import annotations

import inspect
import builtins
from dataclasses import fields, replace
import hashlib
import multiprocessing
import os
import signal
import sys
import threading
import time
from pathlib import Path
from types import SimpleNamespace

import pytest
from molt.target_python import TargetPythonVersion, _parse_target_python_version

_REPO_ROOT = Path(__file__).resolve().parents[1]
for _p in (str(_REPO_ROOT), str(_REPO_ROOT / "tests"), str(_REPO_ROOT / "src")):
    if _p not in sys.path:
        sys.path.insert(0, _p)

import molt_diff  # noqa: E402
from tools.compat import backends as compat_backends  # noqa: E402
from tools.compat import diff_output_layout  # noqa: E402
from tools import memory_guard  # noqa: E402
from tests.process_guard_common import HUNG_CHILD_SOURCE  # noqa: E402


def _dispatch_view(context):
    """A backend context without its budget, which each backend narrows for
    itself (``BackendExecutionContext.for_backend``); the rest is the one
    context the dispatch hands every backend."""
    return {
        f.name: getattr(context, f.name) for f in fields(context) if f.name != "budget"
    }


@pytest.mark.parametrize("runner", ["native", "adapter"])
def test_guarded_capture_preserves_observable_bytes(runner, tmp_path, monkeypatch):
    stdout = b"result\r\n\x00\xffno final newline"
    stderr = b"\r\nExperimentalWarning: guest\r\n(node:7) guest\n\xfftail"
    command = [
        sys.executable,
        "-c",
        f"import os; os.write(1, {stdout!r}); os.write(2, {stderr!r})",
    ]
    monkeypatch.setenv("MOLT_DIFF_ROOT", str(tmp_path / "diff"))
    monkeypatch.setenv("MOLT_DIFF_TMPDIR", str(tmp_path / "scratch"))
    environment = dict(os.environ)
    if runner == "native":
        result = molt_diff._run_subprocess(command, env=environment, timeout=30)
    else:
        result = compat_backends._guarded_run(
            command,
            prefix="MOLT_COMPAT_WASM_RUN",
            env=environment,
            budget=compat_backends.ItemBudget(
                compat_backends.TimeoutPolicy.from_environment(
                    {"MOLT_TEST_PROCESS_TIMEOUT_SEC": str(30), **environment}
                ),
                None,
            ),
            backend="wasm",
            phase="run",
        )
    assert result.returncode == 0
    assert result.infrastructure_failure is None
    assert result.stdout.encode("utf-8", errors="surrogateescape") == stdout
    assert result.child_stderr.encode("utf-8", errors="surrogateescape") == stderr


def _guarded_command(runner: str, command: list[str], environment: dict[str, str]):
    if runner == "native":
        return molt_diff._run_subprocess(command, env=environment, timeout=120)
    return compat_backends._guarded_run(
        command,
        prefix="MOLT_COMPAT_WASM_RUN",
        env=environment,
        budget=compat_backends.ItemBudget(
            compat_backends.TimeoutPolicy.from_environment(
                {"MOLT_TEST_PROCESS_TIMEOUT_SEC": str(120), **environment}
            ),
            None,
        ),
        backend="wasm",
        phase="run",
    )


@pytest.mark.parametrize("runner", ["native", "adapter"])
def test_suite_stop_ends_the_running_command_with_the_operator_signal(
    runner, tmp_path, monkeypatch
):
    # HF-232: a suite stopped by the operator must end the commands its
    # workers are running, not wait out their budgets, and start no more.
    pid_file = tmp_path / "child.pid"
    command = [
        sys.executable,
        "-c",
        "import os, pathlib\n"
        f"pathlib.Path({str(pid_file)!r}).write_text(str(os.getpid()))\n"
        + HUNG_CHILD_SOURCE,
    ]
    monkeypatch.setenv("MOLT_DIFF_ROOT", str(tmp_path / "diff"))
    monkeypatch.setenv("MOLT_DIFF_TMPDIR", str(tmp_path / "scratch"))
    environment = dict(os.environ)
    stop = multiprocessing.RawValue("i", 0)

    def stop_once_started() -> None:
        deadline = time.monotonic() + 60
        while not pid_file.exists() and time.monotonic() < deadline:
            time.sleep(0.02)
        stop.value = signal.SIGTERM

    stopper = threading.Thread(target=stop_once_started)
    compat_backends.bind_suite_stop(stop)
    try:
        stopper.start()
        started = time.monotonic()
        result = _guarded_command(runner, command, environment)
        elapsed = time.monotonic() - started
        stopper.join()
        child_pid = int(pid_file.read_text(encoding="utf-8"))
        pid_file.unlink()
        not_started = _guarded_command(runner, command, environment)
    finally:
        compat_backends.bind_suite_stop(None)

    assert result.guard_signal == signal.SIGTERM
    assert elapsed < 30, elapsed
    assert child_pid not in memory_guard.sample_processes()
    assert not pid_file.exists() and not_started.guard_signal == signal.SIGTERM
    assert "was not started" in not_started.stderr


def test_wasm_adapter_preserves_complete_runner_output(tmp_path, monkeypatch):
    # This isolates the adapter's output boundary; it does not fake a compiled
    # WASM execution claim. The real guarded capture is exercised above.
    context = compat_backends.BackendExecutionContext(
        molt_cli_python=sys.executable,
        target_python=_parse_target_python_version("3.12"),
        build_profile="dev",
        capabilities="",
        environment={"MOLT_EXT_ROOT": str(tmp_path)},
        budget=compat_backends.ItemBudget(
            compat_backends.TimeoutPolicy.from_environment({}), None
        ),
    )
    stderr = "\r\nExperimentalWarning: guest\n(node:7) guest\n\nno final newline"
    observed = compat_backends.BackendResult("output\r\n", stderr, 0)

    def guarded(command, **kwargs):
        if kwargs["prefix"] == "MOLT_COMPAT_WASM_BUILD":
            (tmp_path / "output_linked.wasm").touch()
            (tmp_path / "manifest.json").write_text("{}", encoding="utf-8")
            return compat_backends.BackendResult("", "", 0)
        assert kwargs["prefix"] == "MOLT_COMPAT_WASM_RUN"
        return observed

    monkeypatch.setattr(compat_backends, "_guarded_run", guarded)
    actual = compat_backends.WasmAdapter()._build_and_run_owned(
        "case.py", context=context, out_dir=tmp_path
    )
    assert actual is observed
    assert actual.stderr == stderr


@pytest.mark.parametrize("exit_code,keep", [(0, False), (7, True)])
def test_wasm_runtime_cwd_uses_owned_leaf_without_rebasing_inputs(
    exit_code, keep, tmp_path, monkeypatch
):
    import json

    monkeypatch.delenv(diff_output_layout.ROOT_ENV, raising=False)
    monkeypatch.delenv(diff_output_layout.IDENTITY_ENV, raising=False)
    environment = dict(os.environ)
    environment.update(
        MOLT_EXT_ROOT=str(tmp_path),
        MOLT_DIFF_ROOT=str(tmp_path / "diff"),
        MOLT_COMPAT_SCRATCH_ROOT=str(tmp_path / "scratch"),
        MOLT_DIFF_KEEP="1" if keep else "0",
        TMPDIR=str(tmp_path / "explicit-temp"),
        TEMP="explicit-relative-temp",
        TMP="explicit-other-temp",
        PYTHONPATH=str(_REPO_ROOT / "src"),
    )
    source = tmp_path / "case.py"
    source.write_text("print('source')\n", encoding="utf-8")
    data = tmp_path / "fixture.data"
    data.write_text("explicit data", encoding="utf-8")
    environment["MOLT_CWD_PROBE_INPUT"] = str(data)
    context = compat_backends.BackendExecutionContext(
        molt_cli_python=sys.executable,
        target_python=_parse_target_python_version("3.12"),
        build_profile="dev",
        capabilities="",
        environment=environment,
        budget=compat_backends.ItemBudget(
            compat_backends.TimeoutPolicy.from_environment(environment), None
        ),
    )
    real_guarded_run = compat_backends._guarded_run
    leaves = []

    def guarded(command, **kwargs):
        for name in ("TMPDIR", "TEMP", "TMP", "PYTHONPATH", "MOLT_CWD_PROBE_INPUT"):
            assert kwargs["env"][name] == environment[name]
        if kwargs["prefix"] == "MOLT_COMPAT_WASM_BUILD":
            assert kwargs["cwd"] == str(_REPO_ROOT)
            assert command[command.index("build") + 1] == str(source)
            leaf = Path(command[command.index("--out-dir") + 1])
            leaves.append(leaf)
            (leaf / "output_linked.wasm").write_bytes(b"build fixture")
            (leaf / "bundle.data").write_text("bundled data", encoding="utf-8")
            (leaf / "manifest.json").write_text(
                json.dumps({"modules": {"bundle": {"path": "bundle.data"}}}),
                encoding="utf-8",
            )
            return compat_backends.BackendResult("", "", 0)
        assert kwargs["prefix"] == "MOLT_COMPAT_WASM_RUN"
        assert Path(command[2]) == compat_backends._RUN_WASM_JS
        assert Path(command[2]).is_absolute()
        assert Path(command[3]) == leaves[-1] / "manifest.json"
        assert command[1] == f"--redirect-warnings={leaves[-1] / 'node-warnings.log'}"
        # Exercise actual guarded child cwd and relative writes, without a
        # compiler/WASI claim. Build/manifest/bundle inputs remain explicit.
        probe = """
import json, os, pathlib, sys
manifest = pathlib.Path(sys.argv[1])
assert pathlib.Path.cwd() == manifest.parent
assert pathlib.Path(sys.argv[2]).is_file()
assert pathlib.Path(os.environ['MOLT_CWD_PROBE_INPUT']).read_text() == 'explicit data'
bundle = json.loads(manifest.read_text())['modules']['bundle']['path']
assert (manifest.parent / bundle).read_text() == 'bundled data'
pathlib.Path('relative-output').write_text('guest output')
print('owned cwd and explicit inputs')
sys.exit(int(sys.argv[3]))
"""
        return real_guarded_run(
            [sys.executable, "-B", "-c", probe, command[3], command[2], str(exit_code)],
            **kwargs,
        )

    monkeypatch.setattr(compat_backends, "_guarded_run", guarded)
    result = compat_backends.WasmAdapter().build_and_run(str(source), context=context)
    assert result.returncode == exit_code
    assert result.infrastructure_failure is None
    assert result.stdout == "owned cwd and explicit inputs\n"
    assert leaves[0].exists() is keep
    if keep:
        assert (leaves[0] / "relative-output").read_text(
            encoding="utf-8"
        ) == "guest output"
    assert source.read_text(encoding="utf-8") == "print('source')\n"
    assert not (tmp_path / "relative-output").exists()


@pytest.mark.parametrize(
    "declared, expected",
    [
        (
            "fs,env,time,random,fs.read",
            "fs.read,fs.write,env.read,env.write,time.wall,random",
        ),
        ("fs.read", "fs.read"),
        ("", ""),
    ],
)
@pytest.mark.parametrize(
    "adapter, artifact",
    [
        (compat_backends.WasmAdapter, "output_linked.wasm"),
        (compat_backends.LlvmAdapter, "case_molt"),
        (compat_backends.LuauAdapter, "case.luau"),
    ],
)
def test_adapter_child_receives_atomic_selected_capabilities(
    declared, expected, adapter, artifact, tmp_path, monkeypatch
):
    monkeypatch.setenv("MOLT_DIFF_ROOT", str(tmp_path / "diff"))
    monkeypatch.setenv("MOLT_DIFF_TMPDIR", str(tmp_path / "scratch"))
    environment = dict(os.environ)
    environment.update(
        MOLT_CAPABILITY_TIER="none", MOLT_CAPABILITIES="poison.inherited"
    )
    context = compat_backends.BackendExecutionContext(
        molt_cli_python=sys.executable,
        target_python=_parse_target_python_version("3.12"),
        build_profile="dev",
        capabilities=declared,
        environment=environment,
        budget=compat_backends.ItemBudget(
            compat_backends.TimeoutPolicy.from_environment(environment), None
        ),
    )
    real_guarded_run = compat_backends._guarded_run

    def guarded(command, **kwargs):
        if kwargs["prefix"].endswith("BUILD"):
            if declared:
                assert command[command.index("--capabilities") + 1] == declared
            (tmp_path / artifact).touch()
            if artifact.endswith(".wasm"):
                (tmp_path / "manifest.json").write_text("{}", encoding="utf-8")
            return compat_backends.BackendResult("", "", 0)
        # No compiler or target execution is claimed: the real guarded child
        # observes the exact environment each adapter would give its artifact.
        assert kwargs["phase"] == (
            "first_run" if adapter is compat_backends.LlvmAdapter else "run"
        )
        return real_guarded_run(
            [
                sys.executable,
                "-c",
                "import os; print(os.environ['MOLT_CAPABILITIES']); "
                "print(os.environ['MOLT_CAPABILITY_TIER'])",
            ],
            **kwargs,
        )

    monkeypatch.setattr(compat_backends, "_guarded_run", guarded)
    result = adapter()._build_and_run_owned(
        "case.py", context=context, out_dir=tmp_path
    )
    assert result.returncode == 0
    assert result.infrastructure_failure is None
    assert result.stdout == expected + "\nnone\n"
    assert context.capabilities == declared
    assert context.environment["MOLT_CAPABILITIES"] == "poison.inherited"
    assert environment["MOLT_CAPABILITIES"] == "poison.inherited"


@pytest.mark.parametrize("exit_code", (0, 7))
def test_wasm_host_warnings_are_retained_without_filtering_child_stderr(
    exit_code, tmp_path, monkeypatch
):
    from molt.node_runtime import resolve_node_runtime

    node = str(
        resolve_node_runtime(source_root=_REPO_ROOT, guard_prefix="MOLT_WASM_TEST").path
    )
    monkeypatch.setenv("MOLT_DIFF_ROOT", str(tmp_path / "diff"))
    monkeypatch.setenv("MOLT_DIFF_TMPDIR", str(tmp_path / "scratch"))
    environment = dict(os.environ)
    for name in ("NODE_NO_WARNINGS", "NODE_OPTIONS", "NODE_REDIRECT_WARNINGS"):
        environment.pop(name, None)
    context = compat_backends.BackendExecutionContext(
        molt_cli_python=sys.executable,
        target_python=_parse_target_python_version("3.12"),
        build_profile="dev",
        capabilities="",
        environment=environment,
        budget=compat_backends.ItemBudget(
            compat_backends.TimeoutPolicy.from_environment(environment), None
        ),
    )
    guest_stderr = b"ExperimentalWarning: guest\r\n(node:7) guest\n\xfftail"
    real_guarded_run = compat_backends._guarded_run

    def guarded(command, **kwargs):
        if kwargs["prefix"].endswith("BUILD"):
            (tmp_path / "output_linked.wasm").touch()
            (tmp_path / "manifest.json").write_text("{}", encoding="utf-8")
            return compat_backends.BackendResult("", "", 0)
        assert command[1].startswith("--redirect-warnings=")
        # Exercise the actual Node warning producer and destination flag, with
        # no WASM/compiler claim; guest-looking bytes must remain untouched.
        probe = (
            "const {WASI} = require('node:wasi'); new WASI({version:'preview1'});"
            "process.emitWarning('other host warning', {code:'MOLT_PROBE'});"
            f"process.stderr.write(Buffer.from({list(guest_stderr)}));"
            f"process.exitCode = {exit_code};"
        )
        return real_guarded_run([node, command[1], "-e", probe], **kwargs)

    monkeypatch.setattr(compat_backends, "_guarded_run", guarded)
    result = compat_backends.WasmAdapter()._build_and_run_owned(
        "case.py", context=context, out_dir=tmp_path
    )
    assert result.returncode == exit_code
    assert result.infrastructure_failure is None
    assert result.child_stderr.encode("utf-8", errors="surrogateescape") == guest_stderr
    warnings = (tmp_path / "node-warnings.log").read_text(encoding="utf-8")
    assert "WASI is an experimental feature" in warnings
    assert "[MOLT_PROBE] Warning: other host warning" in warnings
    assert result.stderr == result.child_stderr + warnings


def test_wasm_host_warning_read_failure_cannot_pass(tmp_path, monkeypatch):
    context = compat_backends.BackendExecutionContext(
        molt_cli_python=sys.executable,
        target_python=_parse_target_python_version("3.12"),
        build_profile="dev",
        capabilities="",
        environment={"MOLT_EXT_ROOT": str(tmp_path)},
        budget=compat_backends.ItemBudget(
            compat_backends.TimeoutPolicy.from_environment({}), None
        ),
    )

    def guarded(command, **kwargs):
        if kwargs["prefix"].endswith("BUILD"):
            (tmp_path / "output_linked.wasm").touch()
            (tmp_path / "manifest.json").write_text("{}", encoding="utf-8")
        else:
            warning_path = Path(command[1].split("=", 1)[1])
            warning_path.unlink()
            warning_path.mkdir()
        return compat_backends.BackendResult("output", "guest error", 0)

    monkeypatch.setattr(compat_backends, "_guarded_run", guarded)
    result = compat_backends.WasmAdapter()._build_and_run_owned(
        "case.py", context=context, out_dir=tmp_path
    )
    assert result.infrastructure_failure is not None
    assert result.infrastructure_failure.phase == "temporary_artifact_custody"
    assert "cannot read Node host warnings" in result.detail
    assert result.stdout == "output"
    assert result.child_stderr == "guest error"


def test_cross_backend_comparison_uses_child_stream_not_guard_diagnostics():
    reference = compat_backends.BackendResult(
        "output", "first guard diagnostic", 0, child_stderr="guest\r\n"
    )
    candidate = compat_backends.BackendResult(
        "output", "second guard diagnostic", 0, child_stderr="guest\r\n"
    )
    outcomes = {"native": reference, "wasm": candidate}
    assert (
        molt_diff._cross_backend_divergence(
            outcomes, stdout_mode="exact", stderr_mode="exact"
        )
        is None
    )
    outcomes["wasm"] = replace(candidate, child_stderr="guest\n")
    assert "stderr mismatch" in molt_diff._cross_backend_divergence(
        outcomes, stdout_mode="exact", stderr_mode="exact"
    )


def test_backend_receipt_hashes_exact_child_bytes():
    stdout, stderr = b"\xff\r\n", b"\x00\xfe\r\n"
    context = compat_backends.BackendExecutionContext(
        molt_cli_python=sys.executable,
        target_python=_parse_target_python_version("3.12"),
        build_profile="dev",
        capabilities="",
        environment={},
        budget=compat_backends.ItemBudget(
            compat_backends.TimeoutPolicy.from_environment({}), None
        ),
    )
    result = compat_backends.BackendResult(
        stdout.decode("utf-8", errors="surrogateescape"),
        "guard diagnostics are not guest output",
        0,
        child_stderr=stderr.decode("utf-8", errors="surrogateescape"),
    )
    record = {}
    molt_diff._record_backend_result(
        record,
        file_path="case.py",
        backend="native",
        raw_status="pass",
        expect_molt_fail=False,
        outcome=result,
        context=context,
    )
    row = record["backend_rows"][0]
    assert row["stdout_sha256"] == hashlib.sha256(stdout).hexdigest()
    assert row["stderr_sha256"] == hashlib.sha256(stderr).hexdigest()


def test_adapter_scratch_is_fresh_and_retired(tmp_path, monkeypatch):
    monkeypatch.delenv(diff_output_layout.ROOT_ENV, raising=False)
    monkeypatch.delenv(diff_output_layout.IDENTITY_ENV, raising=False)
    monkeypatch.setenv("MOLT_EXT_ROOT", str(tmp_path))
    monkeypatch.delenv("MOLT_COMPAT_SCRATCH_ROOT", raising=False)
    monkeypatch.delenv("MOLT_DIFF_KEEP", raising=False)
    seen = []

    def run(path):
        seen.append(path)
        assert not (path / "output_linked.wasm").exists()
        (path / "output_linked.wasm").write_bytes(b"current")
        return compat_backends.BackendResult("", "", 0)

    assert (
        compat_backends._with_adapter_scratch(
            "wasm", "case.py", run, environment=os.environ
        ).returncode
        == 0
    )
    assert (
        compat_backends._with_adapter_scratch(
            "wasm", "case.py", run, environment=os.environ
        ).returncode
        == 0
    )
    assert seen[0] != seen[1]
    assert all(not path.exists() for path in seen)


def test_adapter_scratch_keep_and_cleanup_failure_are_visible(tmp_path, monkeypatch):
    monkeypatch.setenv("MOLT_DIFF_ROOT", str(tmp_path / "tmp" / "diff"))
    monkeypatch.delenv(diff_output_layout.ROOT_ENV, raising=False)
    monkeypatch.delenv(diff_output_layout.IDENTITY_ENV, raising=False)
    monkeypatch.setenv("MOLT_EXT_ROOT", str(tmp_path))
    monkeypatch.delenv("MOLT_COMPAT_SCRATCH_ROOT", raising=False)
    monkeypatch.setenv("MOLT_DIFF_KEEP", "1")
    seen = []

    def run(path):
        seen.append(path)
        return compat_backends.BackendResult(None, "build failed", 2, build_failed=True)

    kept = compat_backends._with_adapter_scratch(
        "wasm", "case.py", run, environment=os.environ
    )
    assert kept.returncode == 2 and seen[-1].exists()
    monkeypatch.delenv("MOLT_DIFF_KEEP")
    monkeypatch.setattr(
        diff_output_layout,
        "durable_remove_path",
        lambda *_args, **_kwargs: (_ for _ in ()).throw(OSError("blocked")),
    )
    failed = compat_backends._with_adapter_scratch(
        "wasm", "case.py", run, environment=os.environ
    )
    assert failed.returncode == 2 and failed.build_failed
    assert failed.stderr == "build failed"
    assert failed.infrastructure_failure is not None
    assert "cleanup failed" in failed.infrastructure_failure.details[-1]
    assert seen[-1].exists()
    assert (tmp_path / "tmp" / "diff" / "guest_output_cleanup_failures.jsonl").exists()


def test_adapter_test_supplied_directory_is_not_retired(tmp_path, monkeypatch):
    monkeypatch.setattr(
        compat_backends,
        "_with_adapter_scratch",
        lambda _backend, _file, run, **_kwargs: run(tmp_path),
    )
    context = compat_backends.BackendExecutionContext(
        molt_cli_python=sys.executable,
        target_python=_parse_target_python_version("3.12"),
        build_profile="dev",
        capabilities="",
        environment={},
        budget=compat_backends.ItemBudget(
            compat_backends.TimeoutPolicy.from_environment({}), None
        ),
    )
    monkeypatch.setattr(
        compat_backends.WasmAdapter,
        "_build_and_run_owned",
        lambda *_args, **_kwargs: compat_backends.BackendResult("", "", 0),
    )
    assert (
        compat_backends.WasmAdapter()
        .build_and_run("case.py", context=context)
        .returncode
        == 0
    )
    assert tmp_path.exists()


_COMPAT_GUARD_PHASES = (
    "MOLT_COMPAT_WASM_BUILD",
    "MOLT_COMPAT_WASM_RUN",
    "MOLT_COMPAT_LLVM_BUILD",
    "MOLT_COMPAT_LLVM_RUN",
    "MOLT_COMPAT_LUAU_BUILD",
    "MOLT_COMPAT_LUAU_RUN",
)


@pytest.mark.parametrize("prefix", _COMPAT_GUARD_PHASES)
def test_compat_backend_timeouts_use_shared_guard_authority(
    prefix: str, monkeypatch: pytest.MonkeyPatch
) -> None:
    from tools import harness_memory_guard

    captured: dict[str, object] = {}

    def fake_guarded_completed_process(command, **kwargs):
        captured.update(kwargs)
        return SimpleNamespace(stdout="", stderr="", returncode=0, timed_out=False)

    monkeypatch.setattr(
        harness_memory_guard,
        "guarded_completed_process",
        fake_guarded_completed_process,
    )
    compat_backends._guarded_run(
        ["noop"],
        prefix=prefix,
        env={f"{prefix}_TIMEOUT_SEC": "1234"},
        budget=compat_backends.ItemBudget(
            compat_backends.TimeoutPolicy.from_environment(
                {
                    "MOLT_TEST_PROCESS_TIMEOUT_SEC": str(60.0),
                    **{f"{prefix}_TIMEOUT_SEC": "1234"},
                }
            ),
            None,
        ),
        backend=prefix.split("_")[2].lower(),
        phase=prefix.split("_")[-1].lower(),
    )
    assert captured["prefix"] == prefix
    assert captured["timeout"] == 1234.0


def test_compat_backend_timeout_family_has_no_private_parser() -> None:
    source = inspect.getsource(compat_backends)
    assert "def _build_timeout(" not in source
    for prefix in _COMPAT_GUARD_PHASES:
        assert f'prefix="{prefix}"' in source


def test_compat_backend_timeout_accepts_shared_process_fallback(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    from tools import harness_memory_guard

    captured: dict[str, object] = {}

    def fake_guarded_completed_process(command, **kwargs):
        captured.update(kwargs)
        return SimpleNamespace(stdout="", stderr="", returncode=0, timed_out=False)

    monkeypatch.setattr(
        harness_memory_guard,
        "guarded_completed_process",
        fake_guarded_completed_process,
    )
    compat_backends._guarded_run(
        ["noop"],
        prefix="MOLT_COMPAT_WASM_BUILD",
        env={"MOLT_TEST_PROCESS_TIMEOUT_SEC": "1800"},
        budget=compat_backends.ItemBudget(
            compat_backends.TimeoutPolicy.from_environment(
                {
                    "MOLT_TEST_PROCESS_TIMEOUT_SEC": str(600.0),
                    **{"MOLT_TEST_PROCESS_TIMEOUT_SEC": "1800"},
                }
            ),
            None,
        ),
        backend="wasm",
        phase="build",
    )
    assert captured["timeout"] == 1800.0


# ---------------------------------------------------------------------------
# A fake in-memory backend adapter: returns a scripted result, no real build.
# ---------------------------------------------------------------------------


class _FakeAdapter:
    def __init__(self, name: str, result: compat_backends.BackendResult) -> None:
        self.name = name
        self._result = result
        self.contexts: list[compat_backends.BackendExecutionContext] = []

    def availability(self, budget=None) -> compat_backends.BackendAvailability:
        return compat_backends.BackendAvailability(available=True)

    def build_and_run(
        self,
        file_path: str,
        *,
        context: compat_backends.BackendExecutionContext,
    ) -> compat_backends.BackendResult:
        del file_path
        self.contexts.append(context)
        return self._result


def test_backend_execution_context_requires_canonical_target_authority() -> None:
    with pytest.raises(TypeError, match="TargetPythonVersion"):
        compat_backends.BackendExecutionContext(  # type: ignore[arg-type]
            molt_cli_python=sys.executable,
            target_python="3.14",
            build_profile="dev",
            capabilities="",
            environment={},
            budget=compat_backends.ItemBudget(
                compat_backends.TimeoutPolicy.from_environment({}), None
            ),
        )
    with pytest.raises(ValueError, match="must be canonical"):
        compat_backends.BackendExecutionContext(
            molt_cli_python=sys.executable,
            target_python=TargetPythonVersion(3, 14, 1),
            build_profile="dev",
            capabilities="",
            environment={},
            budget=compat_backends.ItemBudget(
                compat_backends.TimeoutPolicy.from_environment({}), None
            ),
        )


@pytest.mark.parametrize("target_python", ("3.13", "3.14"))
@pytest.mark.parametrize("backend", ("wasm", "llvm", "luau", "native"))
@pytest.mark.parametrize("stdlib_profile", (None, "micro", "full"))
def test_cross_backend_build_command_binds_target_python(
    monkeypatch: pytest.MonkeyPatch,
    tmp_path: Path,
    target_python: str,
    backend: str,
    stdlib_profile: str | None,
) -> None:
    selected = str(tmp_path / "selected CLI python")
    monkeypatch.setenv("MOLT_DIFF_MOLT_PYTHON", "conflicting-ambient-python")
    original_import = builtins.__import__

    def import_without_harness_alias(
        name, globals=None, locals=None, fromlist=(), level=0
    ):
        if name == "molt_diff":
            pytest.fail("backend selection must not import the running harness")
        return original_import(name, globals, locals, fromlist, level)

    monkeypatch.setattr(builtins, "__import__", import_without_harness_alias)
    context = compat_backends.BackendExecutionContext(
        molt_cli_python=selected,
        target_python=_parse_target_python_version(target_python),
        build_profile="release",
        capabilities="fs.read",
        environment={
            "MOLT_CAPABILITY_TIER": "none",
            "MOLT_DIFF_STDLIB_PROFILE": stdlib_profile or "",
        },
        budget=compat_backends.ItemBudget(
            compat_backends.TimeoutPolicy.from_environment(
                {
                    "MOLT_CAPABILITY_TIER": "none",
                    "MOLT_DIFF_STDLIB_PROFILE": stdlib_profile or "",
                }
            ),
            None,
        ),
    )

    command = compat_backends._build_cmd(
        "case.py",
        backend,
        tmp_path,
        context,
    )

    # JIT also uses this builder, with the native target and its tier arguments.
    assert command[:4] == [selected, "-m", "molt.cli", "build"]

    assert command.count("--python-version") == 1
    assert command[command.index("--python-version") + 1] == target_python
    assert command[command.index("--build-profile") + 1] == "release"
    assert command[command.index("--capabilities") + 1] == "fs.read"
    assert context.stdlib_profile == stdlib_profile
    if stdlib_profile is None:
        assert "--stdlib-profile" not in command
    else:
        assert command.count("--stdlib-profile") == 1
        assert command[command.index("--stdlib-profile") + 1] == stdlib_profile


def test_backend_execution_context_rejects_invalid_stdlib_profile() -> None:
    with pytest.raises(ValueError, match="MOLT_DIFF_STDLIB_PROFILE"):
        compat_backends.BackendExecutionContext(
            molt_cli_python=sys.executable,
            target_python=_parse_target_python_version("3.12"),
            build_profile="dev",
            capabilities="",
            environment={"MOLT_DIFF_STDLIB_PROFILE": "wide"},
            budget=compat_backends.ItemBudget(
                compat_backends.TimeoutPolicy.from_environment(
                    {"MOLT_DIFF_STDLIB_PROFILE": "wide"}
                ),
                None,
            ),
        )


def test_requested_differential_receipt_cannot_fail_silently(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    blocked_parent = tmp_path / "not-a-directory"
    blocked_parent.write_text("occupied", encoding="utf-8")
    monkeypatch.setattr(
        molt_diff, "_diff_results_jsonl_path", lambda: blocked_parent / "receipt.jsonl"
    )
    with pytest.raises(RuntimeError, match="differential result receipt write failed"):
        molt_diff._record_diff_result({"raw_status": "pass", "resolved_status": "pass"})


# ---------------------------------------------------------------------------
# Direct tests of the divergence helper
# ---------------------------------------------------------------------------


def _outcome(stdout, rc=0, stderr=""):
    return compat_backends.BackendResult(stdout=stdout, stderr=stderr, returncode=rc)


def _infrastructure_outcome(*, child_returncode=0, build_failed=False):
    failure = molt_diff.memory_guard.GuardInfrastructureFailure(
        phase="temporary_artifact_custody", details=("invalid retained index",)
    )
    return compat_backends.BackendResult(
        None if build_failed else "partial",
        "guard infrastructure diagnostic",
        child_returncode or molt_diff.memory_guard.INFRASTRUCTURE_RETURN_CODE,
        build_failed=build_failed,
        child_returncode=child_returncode,
        infrastructure_failure=failure,
    )


@pytest.mark.parametrize("expires_during", ["availability", "build"])
@pytest.mark.parametrize("probe_raises", [False, True])
def test_backend_budget_exhaustion_preserves_prior_infrastructure_and_all_rows(
    fake_test_file, install_fake_registry, monkeypatch, expires_during, probe_raises
):
    clock = SimpleNamespace(now=0.0)
    monkeypatch.setattr(
        compat_backends, "time", SimpleNamespace(monotonic=lambda: clock.now)
    )
    policy = compat_backends.TimeoutPolicy.from_environment({})
    budget = compat_backends.ItemBudget(policy, 10.0)
    original = _infrastructure_outcome(build_failed=True)
    registry, _ = install_fake_registry(
        {"native": original, "llvm": _outcome("42\n"), "luau": _outcome("42\n")}
    )
    calls = []

    def exhaust(phase, supplied_budget):
        assert supplied_budget is budget
        calls.append(phase)
        if phase == expires_during:
            clock.now = 10.0
            if probe_raises:
                supplied_budget.check(build_failed=True)

    def availability(supplied_budget):
        exhaust("availability", supplied_budget)
        return compat_backends.BackendAvailability(True)

    def build(file_path, *, context):
        exhaust("build", context.budget)
        return budget.exhausted_result(build_failed=True)

    monkeypatch.setattr(registry["llvm"], "availability", availability)
    monkeypatch.setattr(registry["llvm"], "build_and_run", build)
    records = []
    monkeypatch.setattr(molt_diff, "_record_diff_result", records.append)
    actual = molt_diff.diff_test(
        str(fake_test_file), targets=("native", "llvm", "luau"), item_budget=budget
    )
    assert actual["resolved_status"] == "uncalibrated"
    assert actual["reason_tag"] == "infrastructure_error"
    rows = records[0]["backend_rows"]
    assert [row["backend"] for row in rows] == ["native", "llvm", "luau"]
    assert rows[0]["infrastructure_failure"] is not None
    assert rows[0]["raw_status"] == "uncalibrated"
    assert all(row["timed_out"] and row["build_failed"] for row in rows[1:])
    assert calls == (
        ["availability"]
        if expires_during == "availability"
        else ["availability", "build"]
    )


@pytest.mark.parametrize("child_returncode", [0, 7, 137])
def test_guarded_adapter_keeps_child_and_infrastructure_failure(
    monkeypatch, child_returncode
):
    expected = _infrastructure_outcome(child_returncode=child_returncode)
    monkeypatch.setattr(
        molt_diff.harness_memory_guard,
        "guarded_completed_process",
        lambda *_args, **_kwargs: expected,
    )
    actual = compat_backends._guarded_run(
        ["fixture"],
        prefix="MOLT_COMPAT_WASM_RUN",
        env={},
        budget=compat_backends.ItemBudget(
            compat_backends.TimeoutPolicy.from_environment(
                {"MOLT_TEST_PROCESS_TIMEOUT_SEC": str(60), **{}}
            ),
            None,
        ),
        backend="wasm",
        phase="run",
    )
    assert actual == expected
    assert (
        actual.as_build_failure(
            detail="build guard failed", fallback="failed"
        ).infrastructure_failure
        is expected.infrastructure_failure
    )


def test_no_divergence_when_backends_agree() -> None:
    per_backend = {
        "native": _outcome("42\n"),
        "wasm": _outcome("42\n"),
    }
    assert (
        molt_diff._cross_backend_divergence(
            per_backend, stdout_mode="exact", stderr_mode="ignore"
        )
        is None
    )


def test_divergence_detected_when_backends_disagree() -> None:
    per_backend = {
        "native": _outcome("42\n"),
        "wasm": _outcome("43\n"),  # the fork
    }
    detail = molt_diff._cross_backend_divergence(
        per_backend, stdout_mode="exact", stderr_mode="ignore"
    )
    assert detail is not None
    assert "native != wasm" in detail


def test_divergence_detected_on_exit_code_fork() -> None:
    per_backend = {
        "native": _outcome("x\n", rc=0),
        "wasm": _outcome("x\n", rc=1),  # same stdout, different exit code
    }
    detail = molt_diff._cross_backend_divergence(
        per_backend, stdout_mode="exact", stderr_mode="ignore"
    )
    assert detail is not None
    assert "exit code" in detail


def test_single_backend_never_diverges() -> None:
    per_backend = {"native": _outcome("42\n")}
    assert (
        molt_diff._cross_backend_divergence(
            per_backend, stdout_mode="exact", stderr_mode="ignore"
        )
        is None
    )


def test_build_failed_backend_excluded_from_cross_check() -> None:
    # A build-failed backend (stdout=None) is judged by its CPython verdict, not
    # the cross-backend check; with only one backend producing output there is no
    # pair to diverge.
    per_backend = {
        "native": _outcome("42\n"),
        "wasm": _outcome(None, rc=1, stderr="wasm build failed"),
    }
    assert (
        molt_diff._cross_backend_divergence(
            per_backend, stdout_mode="exact", stderr_mode="ignore"
        )
        is None
    )


def test_exact_twins_compare_stderr_even_when_the_file_ignores_it() -> None:
    # native and jit share frontend output, codegen and runtime bytes; only the
    # code owner differs, so a stderr fork between them is a code-owner defect.
    per_backend = {
        "native": _outcome("42\n", stderr="Traceback: ValueError\n"),
        "jit": _outcome("42\n", stderr="Traceback: KeyError\n"),
    }
    detail = molt_diff._cross_backend_divergence(
        per_backend, stdout_mode="exact", stderr_mode="ignore"
    )
    assert detail is not None
    assert "native != jit" in detail


def test_non_twins_keep_the_file_stderr_law() -> None:
    per_backend = {
        "native": _outcome("42\n", stderr="native diagnostic\n"),
        "wasm": _outcome("42\n", stderr="wasm diagnostic\n"),
    }
    assert (
        molt_diff._cross_backend_divergence(
            per_backend, stdout_mode="exact", stderr_mode="ignore"
        )
        is None
    )


def test_twins_agreeing_exactly_do_not_diverge() -> None:
    per_backend = {
        "native": _outcome("42\n", rc=3, stderr="same\n"),
        "jit": _outcome("42\n", rc=3, stderr="same\n"),
    }
    assert (
        molt_diff._cross_backend_divergence(
            per_backend, stdout_mode="exact", stderr_mode="ignore"
        )
        is None
    )


def test_twin_that_fails_to_build_alone_is_a_divergence() -> None:
    per_backend = {
        "native": _outcome("42\n"),
        "jit": compat_backends.BackendResult(
            None, "jit build failed", 1, build_failed=True
        ),
    }
    detail = molt_diff._cross_backend_divergence(
        per_backend, stdout_mode="exact", stderr_mode="ignore"
    )
    assert detail is not None
    assert "native built but its exact twin jit did not" in detail


def test_a_twin_build_that_timed_out_is_unknown_not_a_divergence() -> None:
    # Under load the JIT tier can spend its run budget compiling; the timeout
    # fails the file against CPython but proves nothing about either build.
    per_backend = {
        "native": _outcome("42\n"),
        "jit": compat_backends.BackendResult(
            None, "jit compilation timed out", 124, build_failed=True, timed_out=True
        ),
    }
    assert (
        molt_diff._cross_backend_divergence(
            per_backend, stdout_mode="exact", stderr_mode="ignore"
        )
        is None
    )


@pytest.mark.parametrize("started", [True, False])
def test_jit_adapter_counts_a_runner_that_never_started_as_a_failed_build(
    started, tmp_path, monkeypatch
):
    # The JIT tier compiles when it runs, so only the runner's start receipt
    # tells a compile failure from the program's own exit status 125. This
    # isolates the adapter boundary; the twin gate runs the real runner.
    import json

    from molt.cli import run_tier

    context = compat_backends.BackendExecutionContext(
        molt_cli_python=sys.executable,
        target_python=_parse_target_python_version("3.12"),
        build_profile="dev",
        capabilities="",
        environment={"MOLT_EXT_ROOT": str(tmp_path)},
        budget=compat_backends.ItemBudget(
            compat_backends.TimeoutPolicy.from_environment({}), None
        ),
    )
    image = tmp_path / f"case_molt{run_tier.JIT_IMAGE_SUFFIX}"
    runs: list[list[str]] = []

    def guarded(command, **kwargs):
        if kwargs["prefix"] == "MOLT_COMPAT_JIT_BUILD":
            assert command[0] == context.molt_cli_python
            assert command[command.index("--output") + 1] == str(image)
            image.write_text("#!/bin/molt-jit --image\n", encoding="utf-8")
            payload = {"data": {"artifacts": {"runner": "/bin/molt-jit"}}}
            return compat_backends.BackendResult(json.dumps(payload), "", 0)
        assert kwargs["prefix"] == "MOLT_COMPAT_JIT_RUN"
        runs.append(command)
        receipt = Path(command[command.index("--start-receipt") + 1])
        if started:
            receipt.write_text(run_tier.JIT_START_RECEIPT, encoding="utf-8")
        return compat_backends.BackendResult("partial\n", "molt-jit: refused\n", 125)

    monkeypatch.setattr(compat_backends, "_guarded_run", guarded)
    result = compat_backends.JitAdapter()._build_and_run_owned(
        "case.py", context=context, out_dir=tmp_path
    )
    # The image is the program's argv[0], so its sys.executable re-runs it.
    assert len(runs) == 1 and runs[0][-2:] == ["--image", str(image)]
    assert result.returncode == 125
    assert result.build_failed is not started
    assert "molt-jit: refused" in result.stderr
    assert (result.stdout == "partial\n") is started


def test_twins_ignore_only_text_that_differs_between_runs_of_one_program() -> None:
    # Two runs of one AOT executable already differ in heap addresses (ASLR)
    # and in the OS thread id of a Rust panic; neither is a code-owner fact.
    native = (
        "Exception ignored in: <function C.__del__ at 0x5ce89121e18>\n"
        "thread '<unnamed>' (352545897) panicked at src/a.rs:1:9:\n"
    )
    jit = (
        "Exception ignored in: <function C.__del__ at 0x43cc4d22018>\n"
        "thread '<unnamed>' (352576206) panicked at src/a.rs:1:9:\n"
    )
    agree = {
        "native": _outcome("x\n", stderr=native),
        "jit": _outcome("x\n", stderr=jit),
    }
    assert (
        molt_diff._cross_backend_divergence(
            agree, stdout_mode="exact", stderr_mode="ignore"
        )
        is None
    )
    moved = {
        "native": _outcome("x\n", stderr=native),
        "jit": _outcome("x\n", stderr=jit.replace("src/a.rs:1:9", "src/a.rs:2:9")),
    }
    assert molt_diff._cross_backend_divergence(
        moved, stdout_mode="exact", stderr_mode="ignore"
    )


def test_twin_stable_text_rewrites_only_process_volatile_spans() -> None:
    assert (
        compat_backends.twin_stable_text(
            "<object at 0xABCdef12> 0x10 thread 'main' (7) panicked"
        )
        == "<object at 0x...> 0x10 thread 'main' panicked"
    )


def test_jit_run_cap_covers_its_compilation() -> None:
    policy = compat_backends.TimeoutPolicy.from_environment({})
    assert policy.phase("jit", "run") == 600.0
    assert policy.phase("llvm", "run") == 60.0


def test_twins_compare_with_each_other_not_only_with_the_first_backend() -> None:
    # wasm is listed first, so the twin pair is not the reference pair.
    per_backend = {
        "wasm": _outcome("42\n"),
        "native": _outcome("42\n", stderr="a\n"),
        "jit": _outcome("42\n", stderr="b\n"),
    }
    detail = molt_diff._cross_backend_divergence(
        per_backend, stdout_mode="exact", stderr_mode="ignore"
    )
    assert detail is not None
    assert "native != jit" in detail


# ---------------------------------------------------------------------------
# Full diff_test multi-backend path with a fake registry (no real build).
# ---------------------------------------------------------------------------


@pytest.fixture
def fake_test_file(tmp_path) -> Path:
    f = tmp_path / "prog.py"
    f.write_text("print(42)\n", encoding="utf-8")
    return f


@pytest.fixture
def cpython_oracle(monkeypatch) -> TargetPythonVersion:
    """Declare the stubbed CPython oracle's identity instead of probing one.

    run_cpython is stubbed, so no live interpreter answers for the oracle; the
    real probe is a guarded subprocess whose successful result is cached for
    the whole process. The declared minor is supported but differs
    from the interpreter running pytest, so a target derived from the host
    rather than the oracle cannot pass. Target admission itself stays real.
    """
    oracle = _parse_target_python_version(
        "3.12" if sys.version_info[:2] == (3, 13) else "3.13"
    )
    command = molt_diff._resolve_python_command(sys.executable)

    def metadata(probed: tuple[str, ...], *, budget=None):
        assert probed == command, f"probed {probed}, not the oracle {command}"
        return molt_diff._PythonOracleMetadata(oracle.feature_version, "oracle-python")

    monkeypatch.setattr(molt_diff, "_python_command_metadata", metadata)
    return oracle


@pytest.fixture
def install_fake_registry(monkeypatch, cpython_oracle):
    """Install a fake backend registry into molt_diff and stub run_cpython.

    Returns a function that takes a mapping {backend: BackendResult}, installs it
    as the registry, and stubs CPython to a chosen oracle output whose identity
    is the declared ``cpython_oracle``.
    """

    def _install(backend_results: dict, cpython=("42\n", "", 0)):
        registry = {
            name: _FakeAdapter(name, result) for name, result in backend_results.items()
        }
        native_contexts: list[compat_backends.BackendExecutionContext] = []
        # Exercise the real native adapter with an in-memory build boundary,
        # while the other adapters record the same context directly.
        native_result = backend_results.get("native")

        def _fake_run_molt(file_path, build_profile, **kwargs):
            assert native_result is not None, "native result must be provided"
            context = kwargs.get("execution_context")
            assert isinstance(context, compat_backends.BackendExecutionContext)
            assert context.build_profile == build_profile
            native_contexts.append(context)
            return native_result

        monkeypatch.setattr(molt_diff, "run_molt", _fake_run_molt)
        if native_result is not None:
            registry["native"] = compat_backends.NativeAdapter(_fake_run_molt)
        monkeypatch.setattr(molt_diff, "_COMPAT_BACKEND_REGISTRY", registry)
        cpython_result = (
            cpython
            if isinstance(cpython, compat_backends.BackendResult)
            else compat_backends.BackendResult(*cpython)
        )
        monkeypatch.setattr(molt_diff, "run_cpython", lambda *a, **k: cpython_result)
        return registry, native_contexts

    return _install


@pytest.mark.parametrize("profile_source", ("environment", "metadata"))
def test_all_backends_receive_one_stdlib_profile(
    fake_test_file: Path,
    install_fake_registry,
    monkeypatch: pytest.MonkeyPatch,
    profile_source: str,
) -> None:
    if profile_source == "environment":
        monkeypatch.setenv("MOLT_DIFF_STDLIB_PROFILE", "full")
    else:
        monkeypatch.delenv("MOLT_DIFF_STDLIB_PROFILE", raising=False)
        fake_test_file.write_text(
            "# MOLT_META: stdlib_profile=full\nprint(42)\n", encoding="utf-8"
        )
    targets = ("native", "wasm", "llvm", "luau")
    registry, native_contexts = install_fake_registry(
        {target: compat_backends.BackendResult("42\n", "", 0) for target in targets}
    )
    records: list[dict[str, object]] = []
    monkeypatch.setattr(molt_diff, "_record_diff_result", records.append)
    assert (
        molt_diff.diff_test(str(fake_test_file), targets=targets)["resolved_status"]
        == "pass"
    )
    context = native_contexts[0]
    assert context.stdlib_profile == "full"
    for target in targets[1:]:
        assert [_dispatch_view(c) for c in registry[target].contexts] == [
            _dispatch_view(context)
        ]
    backend_rows = records[0]["backend_rows"]
    assert isinstance(backend_rows, list)
    assert len(backend_rows) == len(targets)
    assert all(row["stdlib_profile"] == "full" for row in backend_rows)


@pytest.mark.parametrize("inherited_target_overrides", [False, True])
def test_all_backends_receive_one_explicit_untrusted_test_context(
    fake_test_file: Path,
    install_fake_registry,
    cpython_oracle: TargetPythonVersion,
    monkeypatch: pytest.MonkeyPatch,
    inherited_target_overrides: bool,
) -> None:
    fake_test_file.write_text(
        "# MOLT_ENV: MOLT_CAPABILITIES=net\nprint(42)\n",
        encoding="utf-8",
    )
    selected_cli_python = "/selected/context CLI python"
    monkeypatch.setattr(
        molt_diff, "_resolve_molt_cli_python", lambda: selected_cli_python
    )
    monkeypatch.setenv("MOLT_CAPABILITY_TIER", "full")
    monkeypatch.setenv("MOLT_CAPABILITIES", "poison.inherited")
    retired_overrides = {
        "MOLT_PYTHON_VERSION": "9.99",
        "MOLT_SYS_VERSION_INFO": "not,a,version",
        "MOLT_SYS_VERSION": "ambient version",
        "MOLT_SYS_API_VERSION": "9999",
        "MOLT_SYS_ABIFLAGS": "ambient flags",
        "MOLT_SYS_IMPLEMENTATION_NAME": "ambient implementation",
        "MOLT_SYS_CACHE_TAG": "ambient tag",
    }
    for name, value in retired_overrides.items():
        if inherited_target_overrides:
            monkeypatch.setenv(name, value)
        else:
            monkeypatch.delenv(name, raising=False)
    monkeypatch.setenv("PYTHONOPTIMIZE", "2")
    targets = ("native", "wasm", "llvm", "luau")
    registry, native_contexts = install_fake_registry(
        {target: compat_backends.BackendResult("42\n", "", 0) for target in targets}
    )

    status = molt_diff.diff_test(
        str(fake_test_file),
        targets=targets,
        target_python=cpython_oracle.short,
    )["resolved_status"]

    assert status == "pass"
    assert len(native_contexts) == 1
    context = native_contexts[0]
    assert context.molt_cli_python == selected_cli_python
    for target in targets[1:]:
        assert [_dispatch_view(c) for c in registry[target].contexts] == [
            _dispatch_view(context)
        ]
    assert context.target_python == cpython_oracle
    assert context.environment["MOLT_SYS_EXECUTABLE"] == "oracle-python"
    assert context.environment["PYTHONOPTIMIZE"] == "2"
    assert {
        name: context.environment[name]
        for name in retired_overrides
        if name in context.environment
    } == (retired_overrides if inherited_target_overrides else {})
    assert context.environment["MOLT_CAPABILITY_TIER"] == "none"
    assert context.capabilities == "net"
    assert "poison.inherited" not in context.capabilities


def test_target_mismatching_the_oracle_fails_before_any_execution(
    fake_test_file: Path,
    install_fake_registry,
    cpython_oracle: TargetPythonVersion,
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    registry, native_contexts = install_fake_registry(
        {
            "native": compat_backends.BackendResult("42\n", "", 0),
            "wasm": compat_backends.BackendResult("42\n", "", 0),
        }
    )
    monkeypatch.setattr(
        molt_diff,
        "run_cpython",
        lambda *_a, **_k: pytest.fail("an inadmissible target ran the oracle"),
    )
    # 3.14 is a supported target but never the declared oracle.
    assert cpython_oracle.short != "3.14"
    with pytest.raises(ValueError, match="does not match the CPython oracle"):
        molt_diff.diff_test(
            str(fake_test_file), targets=("native", "wasm"), target_python="3.14"
        )["resolved_status"]
    assert native_contexts == []
    assert registry["wasm"].contexts == []


def test_all_backends_agree_with_cpython_passes(
    fake_test_file, install_fake_registry
) -> None:
    install_fake_registry(
        {
            "native": compat_backends.BackendResult("42\n", "", 0),
            "wasm": compat_backends.BackendResult("42\n", "", 0),
        },
        cpython=("42\n", "", 0),
    )
    status = molt_diff.diff_test(str(fake_test_file), targets=("native", "wasm"))[
        "resolved_status"
    ]
    assert status == "pass"


@pytest.mark.parametrize(
    "guest_bytes", [b"", b"ExperimentalWarning: guest\r\n\x00\xfftail"]
)
def test_diff_report_separates_compared_stderr_from_retained_diagnostics(
    guest_bytes, fake_test_file, install_fake_registry, monkeypatch, capsys
) -> None:
    fake_test_file.write_text(
        "# MOLT_META: stderr=exact\nprint(42)\n", encoding="utf-8"
    )
    guest_stderr = guest_bytes.decode("utf-8", errors="surrogateescape")
    diagnostics = "guard preface\n" + guest_stderr + "\nExperimentalWarning: host\n"
    targets = ("native", "wasm", "llvm", "luau")
    result = compat_backends.BackendResult(
        "WRONG\n", diagnostics, 0, child_stderr=guest_stderr
    )
    install_fake_registry(
        {target: result for target in targets}, cpython=("42\n", guest_stderr, 0)
    )
    records = []
    monkeypatch.setattr(molt_diff, "_record_diff_result", records.append)

    assert (
        molt_diff.diff_test(str(fake_test_file), targets=targets)["resolved_status"]
        == "fail"
    )

    lines = capsys.readouterr().out.splitlines()
    assert lines.count(f"  Molt    return: 0 stderr: {guest_stderr!r}") == len(targets)
    assert lines.count(f"  Molt diagnostics (combined stderr): {diagnostics!r}") == len(
        targets
    )
    assert all(
        row["stderr_sha256"] == hashlib.sha256(guest_bytes).hexdigest()
        for row in records[0]["backend_rows"]
    )

    # Host diagnostics alone must not change the full differential verdict.
    install_fake_registry(
        {target: replace(result, stdout="42\n") for target in targets},
        cpython=("42\n", guest_stderr, 0),
    )
    assert (
        molt_diff.diff_test(str(fake_test_file), targets=targets)["resolved_status"]
        == "pass"
    )
    assert result.stderr == diagnostics
    assert result.child_stderr == guest_stderr


def test_backend_metadata_filters_each_requested_cell_before_execution(
    fake_test_file: Path, install_fake_registry
) -> None:
    fake_test_file.write_text(
        "# MOLT_META: backends=wasm\nprint(42)\n", encoding="utf-8"
    )
    install_fake_registry(
        {"wasm": compat_backends.BackendResult("42\n", "", 0)},
        cpython=("42\n", "", 0),
    )

    status = molt_diff.diff_test(str(fake_test_file), targets=("native", "wasm"))[
        "resolved_status"
    ]

    assert status == "pass"


def test_one_backend_wrong_vs_cpython_fails(
    fake_test_file, install_fake_registry
) -> None:
    # native matches CPython, wasm does not -> the single-backend (native) run
    # would have been GREEN; the multi-backend oracle catches the wasm fork.
    install_fake_registry(
        {
            "native": compat_backends.BackendResult("42\n", "", 0),
            "wasm": compat_backends.BackendResult("WRONG\n", "", 0),
        },
        cpython=("42\n", "", 0),
    )
    # Sanity: native alone is green (the invisible-divergence baseline).
    native_only = molt_diff.diff_test(str(fake_test_file), targets=("native",))[
        "resolved_status"
    ]
    assert native_only == "pass"
    # Multi-backend: RED.
    status = molt_diff.diff_test(str(fake_test_file), targets=("native", "wasm"))[
        "resolved_status"
    ]
    assert status == "fail"


def test_backends_disagree_with_each_other_fails(
    fake_test_file, install_fake_registry
) -> None:
    # Pathological: BOTH backends disagree with CPython, but they also disagree
    # with EACH OTHER. The cross-backend check fails it regardless of CPython.
    install_fake_registry(
        {
            "native": compat_backends.BackendResult("A\n", "", 0),
            "wasm": compat_backends.BackendResult("B\n", "", 0),
        },
        cpython=("Z\n", "", 0),
    )
    status = molt_diff.diff_test(str(fake_test_file), targets=("native", "wasm"))[
        "resolved_status"
    ]
    assert status == "fail"


def test_fault_injection_seam_produces_divergence(
    fake_test_file, install_fake_registry, monkeypatch
) -> None:
    # The fault-injection env hook (used by the heavy E2E proof) perturbs one
    # backend's stdout; the oracle must catch it. Here we drive it through the
    # real adapter fault path by wrapping the fake adapter's result.
    monkeypatch.setenv("MOLT_COMPAT_FAULT_INJECT", "wasm")
    # The fake adapter returns clean output; apply the real injection helper so
    # the seam itself is exercised (not a hand-faked string).
    base = compat_backends.BackendResult("42\n", "", 0)
    injected = compat_backends._apply_fault_injection("wasm", base)
    assert injected.stdout != base.stdout  # the seam fired
    install_fake_registry(
        {
            "native": compat_backends.BackendResult("42\n", "", 0),
            "wasm": injected,
        },
        cpython=("42\n", "", 0),
    )
    status = molt_diff.diff_test(str(fake_test_file), targets=("native", "wasm"))[
        "resolved_status"
    ]
    assert status == "fail"


def test_fault_injection_inert_when_unset() -> None:
    base = compat_backends.BackendResult("42\n", "", 0)
    out = compat_backends._apply_fault_injection("wasm", base)
    assert out.stdout == base.stdout  # no env -> no perturbation


@pytest.mark.usefixtures("cpython_oracle")
def test_uncalibrated_when_no_backend_available(fake_test_file, monkeypatch) -> None:
    # A backend whose toolchain is unavailable is a LOUD uncalibrated, never a
    # silent pass. With only an unavailable backend requested, the test resolves
    # to "uncalibrated".
    class _Unavailable:
        name = "luau"

        def availability(self, budget=None):
            return compat_backends.BackendAvailability(
                available=False, reason="lune not on PATH"
            )

        def build_and_run(self, *a, **k):  # pragma: no cover - never called
            raise AssertionError("unavailable backend must not run")

    monkeypatch.setattr(molt_diff, "_COMPAT_BACKEND_REGISTRY", {"luau": _Unavailable()})
    monkeypatch.setattr(molt_diff, "run_cpython", lambda *a, **k: _outcome("42\n"))
    status = molt_diff.diff_test(str(fake_test_file), targets=("luau",))[
        "resolved_status"
    ]
    assert status == "uncalibrated"


@pytest.mark.parametrize("prefix", _COMPAT_GUARD_PHASES)
def test_timeout_preserves_diagnostic_and_is_never_oom(prefix, monkeypatch) -> None:
    from tools import harness_memory_guard

    def finish(command, **kwargs):
        assert kwargs["text"] is False
        return harness_memory_guard.GuardedCompletedProcess(
            command,
            124,
            b"partial\r\n\xff",
            b"killed\r\n\xff\nmemory_guard: timeout after 60.0s",
            child_stderr=b"killed\r\n\xff",
            elapsed_s=60.1,
            timed_out=True,
            child_returncode=-9,
            guard_signal=9,
        )

    monkeypatch.setattr(harness_memory_guard, "guarded_completed_process", finish)
    result = compat_backends._guarded_run(
        ["noop"],
        prefix=prefix,
        env={},
        budget=compat_backends.ItemBudget(
            compat_backends.TimeoutPolicy.from_environment(
                {"MOLT_TEST_PROCESS_TIMEOUT_SEC": str(60.0), **{}}
            ),
            None,
        ),
        backend=prefix.split("_")[2].lower(),
        phase=prefix.split("_")[-1].lower(),
    )
    assert result.stdout.encode("utf-8", errors="surrogateescape") == b"partial\r\n\xff"
    assert (
        result.child_stderr.encode("utf-8", errors="surrogateescape")
        == b"killed\r\n\xff"
    )
    assert "killed" in result.stderr
    assert "timeout after 60.0s" in result.stderr
    assert result.returncode == 124 and result.timed_out
    assert result.child_returncode == -9 and result.guard_signal == 9
    assert replace(result, child_stderr="MemoryError").resource_failure is None


@pytest.mark.parametrize("backend", ("wasm", "llvm", "luau"))
@pytest.mark.parametrize("phase", ("build", "run"))
@pytest.mark.parametrize("failure_kind", ("timeout", "infrastructure"))
def test_all_adapters_preserve_phase_failure(
    backend, phase, failure_kind, tmp_path, monkeypatch
):
    monkeypatch.setattr(
        compat_backends,
        "_with_adapter_scratch",
        lambda _backend, _file, run, **_kwargs: run(tmp_path),
    )
    context = compat_backends.BackendExecutionContext(
        molt_cli_python=sys.executable,
        target_python=_parse_target_python_version("3.12"),
        build_profile="dev",
        capabilities="",
        environment={"MOLT_EXT_ROOT": str(tmp_path)},
        budget=compat_backends.ItemBudget(
            compat_backends.TimeoutPolicy.from_environment({}), None
        ),
    )
    calls = []

    def guarded(command, **kwargs):
        prefix = kwargs["prefix"]
        calls.append(prefix)
        if prefix.endswith("BUILD") and phase == "run":
            output = {
                "wasm": "output_linked.wasm",
                "llvm": "case_molt",
                "luau": "case.luau",
            }[backend]
            (tmp_path / output).touch()
            if backend == "wasm":
                (tmp_path / "manifest.json").write_text("{}", encoding="utf-8")
            return compat_backends.BackendResult("", "", 0)
        if failure_kind == "infrastructure":
            return _infrastructure_outcome()
        return compat_backends.BackendResult.from_deadline(
            timeout=17.0,
            stdout="partial",
            stderr="deadline diagnostic",
        )

    monkeypatch.setattr(compat_backends, "_guarded_run", guarded)
    adapters = {
        "wasm": compat_backends.WasmAdapter,
        "llvm": compat_backends.LlvmAdapter,
        "luau": compat_backends.LuauAdapter,
    }
    result = adapters[backend]().build_and_run("case.py", context=context)
    if failure_kind == "timeout":
        assert result.timed_out and result.returncode == 124
        assert "deadline diagnostic" in result.stderr
        assert "timeout after 17.0s" in result.stderr
    else:
        assert result.infrastructure_failure is not None
        assert result.child_returncode == 0
        assert result.returncode == molt_diff.memory_guard.INFRASTRUCTURE_RETURN_CODE
    assert result.build_failed == (phase == "build")
    if phase == "build":
        assert result.stdout is None
        assert "partial" in result.stderr
    else:
        assert result.stdout == "partial"
    assert len(calls) == (1 if phase == "build" else 2)
    monkeypatch.setenv("MOLT_COMPAT_FAULT_INJECT", backend)
    injected = compat_backends._apply_fault_injection(backend, result)
    assert injected.timed_out == result.timed_out
    assert injected.returncode == result.returncode
    assert injected.infrastructure_failure is result.infrastructure_failure
    assert injected.stderr == result.stderr


def test_native_adapter_preserves_timeout():
    context = compat_backends.BackendExecutionContext(
        molt_cli_python=sys.executable,
        target_python=_parse_target_python_version("3.12"),
        build_profile="dev",
        capabilities="",
        environment={},
        budget=compat_backends.ItemBudget(
            compat_backends.TimeoutPolicy.from_environment({}), None
        ),
    )
    adapter = compat_backends.NativeAdapter(
        lambda *a, **k: compat_backends.BackendResult(
            None, "deadline", 124, build_failed=True, timed_out=True
        )
    )
    result = adapter.build_and_run("case.py", context=context)
    assert result.timed_out and result.build_failed


def test_native_build_timeout_keeps_partial_compiler_diagnostics():
    result = compat_backends.BackendResult.from_deadline(
        timeout=3,
        stdout=b"phase detail",
        stderr=b"error detail",
        build_failed=True,
    )
    assert result.stdout is None and result.build_failed and result.timed_out
    assert "phase detail" in result.stderr and "error detail" in result.stderr


@pytest.mark.parametrize("backend", ("native", "wasm", "llvm", "luau"))
def test_timeout_cannot_be_xfailed_even_with_other_backend_divergence(
    backend, fake_test_file, install_fake_registry, monkeypatch
):
    fake_test_file.write_text(
        "# MOLT_META: expect_fail=molt expect_fail_reason=semantic_gap\nprint(42)\n",
        encoding="utf-8",
    )
    targets = ("native", "wasm", "llvm", "luau")
    outcomes = {name: _outcome(name) for name in targets}
    outcomes[backend] = compat_backends.BackendResult(
        None, "killed after timeout", 124, build_failed=True, timed_out=True
    )
    install_fake_registry(outcomes)
    records = []
    monkeypatch.setattr(molt_diff, "_record_diff_result", records.append)
    assert (
        molt_diff.diff_test(str(fake_test_file), targets=targets)["resolved_status"]
        == "fail"
    )
    assert records[0]["expect_molt_fail"] is True
    assert records[0]["reason_tag"] == "timeout"
    rows = records[0]["backend_rows"]
    assert len(rows) == 4
    row = next(row for row in rows if row["backend"] == backend)
    assert row["timed_out"] and row["build_failed"]
    assert row["raw_status"] == "fail"


def test_oom_keeps_prior_backend_receipt_and_its_own_diagnostic(
    fake_test_file, install_fake_registry, monkeypatch, capsys
):
    diagnostic = "allocator exhausted its memory budget"
    install_fake_registry(
        {
            "native": _outcome("42\n"),
            "wasm": compat_backends.BackendResult(
                None, diagnostic, 137, build_failed=True, rss_limit_exceeded=True
            ),
        }
    )
    records = []
    monkeypatch.setattr(molt_diff, "_record_diff_result", records.append)
    assert (
        molt_diff.diff_test(str(fake_test_file), targets=("native", "wasm"))[
            "resolved_status"
        ]
        == "oom"
    )
    rows = records[0]["backend_rows"]
    assert [(row["backend"], row["raw_status"]) for row in rows] == [
        ("native", "pass"),
        ("wasm", "oom"),
    ]
    assert rows[1]["stderr_sha256"] == hashlib.sha256(diagnostic.encode()).hexdigest()
    assert diagnostic in capsys.readouterr().out


@pytest.mark.parametrize("native_output", ("42\n", "wrong\n"))
def test_missing_target_is_not_pass_or_expected_semantic_failure(
    native_output, fake_test_file, install_fake_registry, monkeypatch
):
    fake_test_file.write_text(
        "# MOLT_META: expect_fail=molt expect_fail_reason=semantic_gap\nprint(42)\n",
        encoding="utf-8",
    )
    registry, _ = install_fake_registry(
        {"native": _outcome(native_output), "luau": _outcome("")}
    )
    monkeypatch.setattr(
        registry["luau"],
        "availability",
        lambda budget=None: compat_backends.BackendAvailability(
            False, "runner missing"
        ),
    )
    records = []
    monkeypatch.setattr(molt_diff, "_record_diff_result", records.append)
    assert (
        molt_diff.diff_test(str(fake_test_file), targets=("native", "luau"))[
            "resolved_status"
        ]
        == "uncalibrated"
    )
    assert records[0]["backend_rows"][1]["detail"] == "runner missing"
    assert records[0]["backend_rows"][1]["returncode"] is None


def test_cpython_timeout_cannot_become_semantic_parity(
    fake_test_file, install_fake_registry, monkeypatch
):
    fake_test_file.write_text(
        "# MOLT_META: expect_fail=molt expect_fail_reason=semantic_gap\nprint(42)\n",
        encoding="utf-8",
    )
    registry, native_contexts = install_fake_registry(
        {"native": _outcome("", rc=124)},
        cpython=compat_backends.BackendResult(
            "", "oracle deadline", 124, timed_out=True
        ),
    )
    records = []
    monkeypatch.setattr(molt_diff, "_record_diff_result", records.append)
    assert molt_diff.diff_test(str(fake_test_file))["resolved_status"] == "fail"
    assert records[0]["reason_tag"] == "timeout"
    assert native_contexts == []


def test_intentional_exit_124_is_not_a_timeout(fake_test_file, install_fake_registry):
    install_fake_registry(
        {"native": _outcome("", rc=124), "wasm": _outcome("", rc=124)},
        cpython=("", "", 124),
    )
    assert (
        molt_diff.diff_test(str(fake_test_file), targets=("native", "wasm"))[
            "resolved_status"
        ]
        == "pass"
    )


@pytest.mark.parametrize("backend", ("native", "wasm", "llvm", "luau"))
@pytest.mark.parametrize("build_failed", (False, True))
def test_infrastructure_failure_is_not_semantic_or_oom_evidence(
    backend, build_failed, fake_test_file, install_fake_registry, monkeypatch, capsys
):
    fake_test_file.write_text(
        "# MOLT_META: expect_fail=molt expect_fail_reason=semantic_gap\nprint(42)\n",
        encoding="utf-8",
    )
    targets = ("native", "wasm", "llvm", "luau")
    outcomes = {name: _outcome(name) for name in targets}
    failure = _infrastructure_outcome(child_returncode=137, build_failed=build_failed)
    outcomes[backend] = failure
    install_fake_registry(outcomes)
    monkeypatch.setenv("MOLT_COMPAT_FAULT_INJECT", backend)
    records = []
    monkeypatch.setattr(molt_diff, "_record_diff_result", records.append)
    assert (
        molt_diff.diff_test(str(fake_test_file), targets=targets)["resolved_status"]
        == "uncalibrated"
    )
    assert records[0]["reason_tag"] == "infrastructure_error"
    assert records[0]["raw_status"] == records[0]["resolved_status"] == "uncalibrated"
    row = next(row for row in records[0]["backend_rows"] if row["backend"] == backend)
    assert row["raw_status"] == "uncalibrated"
    assert row["child_returncode"] == 137
    assert (
        row["infrastructure_failure"] == failure.infrastructure_failure.json_payload()
    )
    assert "CROSS-BACKEND DIVERGENCE" not in capsys.readouterr().out
    assert (
        molt_diff._cross_backend_divergence(
            {"valid": _outcome("42"), "invalid": failure},
            stdout_mode="exact",
            stderr_mode="ignore",
        )
        is None
    )


@pytest.mark.parametrize("backend", ["cpython", "native", "wasm", "llvm", "luau"])
def test_guest_cleanup_failure_cannot_be_hidden_by_xfail(
    backend, fake_test_file, install_fake_registry, monkeypatch, tmp_path
):
    fake_test_file.write_text(
        "# MOLT_META: expect_fail=molt expect_fail_reason=semantic_gap\nprint(42)\n",
        encoding="utf-8",
    )
    lease = SimpleNamespace(
        path=tmp_path, retire=lambda **_kwargs: "owned guest cleanup failed"
    )
    failure = compat_backends.run_with_guest_outputs(
        [lease], lambda: _outcome("42\n"), environment={}, repo_root=_REPO_ROOT
    )
    assert (failure.stdout, failure.stderr, failure.returncode) == ("42\n", "", 0)
    assert failure.infrastructure_failure is not None
    targets = ("native", "wasm", "llvm", "luau")
    outcomes = {name: _outcome("42\n") for name in targets}
    if backend == "cpython":
        install_fake_registry(outcomes, cpython=failure)
    else:
        outcomes[backend] = failure
        install_fake_registry(outcomes)
    records = []
    monkeypatch.setattr(molt_diff, "_record_diff_result", records.append)
    assert (
        molt_diff.diff_test(str(fake_test_file), targets=targets)["resolved_status"]
        == "uncalibrated"
    )
    assert records[0]["reason_tag"] == "infrastructure_error"
    assert records[0]["raw_status"] == records[0]["resolved_status"] == "uncalibrated"
    payload = (
        records[0]["cpython_infrastructure_failure"]
        if backend == "cpython"
        else next(
            row for row in records[0]["backend_rows"] if row["backend"] == backend
        )["infrastructure_failure"]
    )
    assert payload == failure.infrastructure_failure.json_payload()


def test_cpython_infrastructure_failure_cannot_be_semantic_parity(
    fake_test_file, install_fake_registry, monkeypatch
):
    oracle = _infrastructure_outcome()
    _, contexts = install_fake_registry(
        {"native": _outcome("partial", rc=125)}, cpython=oracle
    )
    records = []
    monkeypatch.setattr(molt_diff, "_record_diff_result", records.append)
    assert molt_diff.diff_test(str(fake_test_file))["resolved_status"] == "uncalibrated"
    assert contexts == []
    assert records[0]["reason_tag"] == "infrastructure_error"
    assert records[0]["cpython_child_returncode"] == 0
    assert (
        records[0]["cpython_infrastructure_failure"]
        == oracle.infrastructure_failure.json_payload()
    )


def test_intentional_exit_125_remains_semantic_evidence(
    fake_test_file, install_fake_registry
):
    install_fake_registry({"native": _outcome("", rc=125)}, cpython=("", "", 125))
    assert molt_diff.diff_test(str(fake_test_file))["resolved_status"] == "pass"


@pytest.mark.parametrize(
    "kind",
    ["infrastructure", "rss", "allocation", "interrupted", "timeout", "dyld", "daemon"],
)
def test_native_first_failure_is_terminal_and_preserves_selection(
    monkeypatch, tmp_path, kind
):
    failure = {
        "infrastructure": _infrastructure_outcome(build_failed=True),
        "rss": compat_backends.BackendResult(
            None, "RSS limit exceeded", 125, build_failed=True, rss_limit_exceeded=True
        ),
        "allocation": compat_backends.BackendResult(
            None, "MemoryError", 1, build_failed=True
        ),
        "interrupted": compat_backends.BackendResult(
            None, "guard interrupted", 143, build_failed=True, guard_signal=15
        ),
        "timeout": compat_backends.BackendResult.from_deadline(
            timeout=5, build_failed=True
        ),
        "dyld": compat_backends.BackendResult(
            None, "dyld: unknown imports format", 1, build_failed=True
        ),
        "daemon": compat_backends.BackendResult(
            None, "backend daemon connection failed", 1, build_failed=True
        ),
    }[kind]
    selected = {
        "MOLT_DIFF_ROOT": str(tmp_path),
        "MOLT_BACKEND_DAEMON": "1",
        "MOLT_DIFF_FORCE_NO_CACHE": "0",
        "MOLT_DIFF_FORCE_REBUILD": "0",
    }
    for key, value in selected.items():
        monkeypatch.setenv(key, value)
    calls = []

    def run(file, profile, *, execution_context, **kwargs):
        calls.append((file, profile, execution_context, kwargs))
        return failure if len(calls) == 1 else _outcome("recovered")

    monkeypatch.setattr(molt_diff, "run_molt", run)
    monkeypatch.setattr(
        molt_diff, "_COMPAT_BACKEND_REGISTRY", compat_backends.build_registry(run)
    )
    context = compat_backends.BackendExecutionContext(
        molt_cli_python=sys.executable,
        target_python=_parse_target_python_version("3.12"),
        build_profile="dev",
        capabilities="",
        environment=selected.copy(),
        budget=compat_backends.ItemBudget(
            compat_backends.TimeoutPolicy.from_environment({}), None
        ),
    )
    assert (
        molt_diff._run_backend_for_diff(
            backend="native", file_path="fixture.py", context=context
        )
        is failure
    )
    assert calls == [("fixture.py", "dev", context, {})]
    assert context.environment == selected
    assert {key: os.environ[key] for key in selected} == selected
    assert not list(tmp_path.iterdir())


@pytest.mark.parametrize("returncode", [0, 1, 9, -9, 137, 0xC0000409])
@pytest.mark.parametrize(
    "diagnostic",
    [
        "RuntimeError: boom",
        'memory_guard: repro context: {"command": ["--no-retry-oom"], "env": {"NOTE": "out of memory"}}',
        "command: compiler --error-label=MemoryError --output=allocation failed",
        '  File "out of memory.py", line 2\n    raise MemoryError',
        "molt fatal: invalid object header in dec_ref\nMemoryError: cleanup failed",
    ],
)
def test_resource_verdict_ignores_context_and_unmeasured_kills(returncode, diagnostic):
    result = compat_backends.BackendResult("", diagnostic, returncode)
    assert result.resource_failure is None


@pytest.mark.parametrize(
    "diagnostic",
    [
        "MemoryError",
        "MemoryError: out of memory",
        "OOM",
        "out of memory",
        "std::bad_alloc",
        "  what():  std::bad_alloc",
        "terminate called after throwing an instance of 'std::bad_alloc'",
        "memory allocation of 4096 bytes failed",
        "LLVM ERROR: out of memory",
        "FATAL ERROR: Reached heap limit Allocation failed - JavaScript heap out of memory",
        "OSError: [Errno 12] Cannot allocate memory",
        "RuntimeError: memory allocation failed",
    ],
)
def test_resource_verdict_retains_allocator_evidence(diagnostic):
    result = compat_backends.BackendResult("", diagnostic + "\n", 1)
    assert result.resource_failure == "allocation_failed"
    assert replace(result, returncode=0).resource_failure is None
    assert replace(result, timed_out=True).resource_failure is None
    assert replace(result, guard_signal=15).resource_failure is None
    assert (
        replace(
            result,
            infrastructure_failure=_infrastructure_outcome().infrastructure_failure,
        ).resource_failure
        is None
    )


@pytest.mark.parametrize("prefix", _COMPAT_GUARD_PHASES)
@pytest.mark.parametrize("measured", [False, True])
def test_guard_resource_facts_survive_adapter_and_build_conversion(
    prefix, measured, monkeypatch
):
    guard = molt_diff.harness_memory_guard
    result = guard.GuardedCompletedProcess(
        ["fixture", "--no-retry-oom"],
        137 if measured else 0xC0000409,
        "MemoryError\n",
        "runtime crashed\nMemoryError\n",
        elapsed_s=0.1,
        child_stderr="runtime crashed\n",
        violation=molt_diff.memory_guard.RssViolation(7, 2048, "fixture", "process")
        if measured
        else None,
        child_returncode=-9 if measured else 0xC0000409,
    )
    monkeypatch.setattr(guard, "guarded_completed_process", lambda *a, **k: result)
    actual = compat_backends._guarded_run(
        ["fixture"],
        prefix=prefix,
        env={},
        budget=compat_backends.ItemBudget(
            compat_backends.TimeoutPolicy.from_environment(
                {"MOLT_TEST_PROCESS_TIMEOUT_SEC": str(5), **{}}
            ),
            None,
        ),
        backend=prefix.split("_")[2].lower(),
        phase=prefix.split("_")[-1].lower(),
    )
    for converted in [
        actual,
        actual.as_build_failure(detail="build failed", fallback="failed"),
    ]:
        assert converted.rss_limit_exceeded is measured
        assert converted.child_returncode == result.child_returncode
        assert converted.child_stderr == "runtime crashed\n"
        assert converted.resource_failure == (
            "rss_limit_exceeded" if measured else None
        )


@pytest.mark.parametrize("backend", ["native", "wasm", "llvm", "luau"])
def test_runtime_abort_with_oom_repro_stays_failure(
    backend, fake_test_file, install_fake_registry, monkeypatch
):
    diagnostic = (
        "molt fatal: invalid object header in dec_ref\n"
        "memory_guard: command exited with NTSTATUS 0xC0000409; no RSS violation observed\n"
        'memory_guard: repro context: {"command": ["--no-retry-oom"], "env": {"OOM": "1"}}\n'
    )
    install_fake_registry(
        {backend: compat_backends.BackendResult("", diagnostic, 0xC0000409)}
    )
    records = []
    monkeypatch.setattr(molt_diff, "_record_diff_result", records.append)
    assert (
        molt_diff.diff_test(str(fake_test_file), targets=(backend,))["resolved_status"]
        == "fail"
    )
    row = records[0]["backend_rows"][0]
    assert row["raw_status"] == "fail" and row["returncode"] == 0xC0000409
    assert row["resource_failure"] is None and not row["rss_limit_exceeded"]
    assert row["stderr_sha256"] == hashlib.sha256(diagnostic.encode()).hexdigest()


@pytest.mark.parametrize("measured", [False, True])
def test_cpython_resource_verdict_uses_same_evidence(
    fake_test_file, install_fake_registry, monkeypatch, measured
):
    oracle = compat_backends.BackendResult(
        "",
        "memory_guard: repro context: --no-retry-oom",
        137,
        rss_limit_exceeded=measured,
    )
    install_fake_registry({"native": _outcome("42\n")}, cpython=oracle)
    records = []
    monkeypatch.setattr(molt_diff, "_record_diff_result", records.append)
    assert molt_diff.diff_test(str(fake_test_file), targets=("native",))[
        "resolved_status"
    ] == ("oom" if measured else "fail")
    assert records[0]["cpython_resource_failure"] == (
        "rss_limit_exceeded" if measured else None
    )


def test_nested_guard_rss_diagnostic_is_evidence_but_repro_mention_is_not():
    line = "memory_guard: RSS limit exceeded; terminated tracked child: pid=7 rss=2.00GB limit=1.00GB"
    result = compat_backends.BackendResult("", line, 125)
    assert result.resource_failure == "rss_limit_exceeded"
    repro = 'memory_guard: repro context: {"command": [' + repr(line) + "]}"
    assert compat_backends.BackendResult("", repro, 125).resource_failure is None


def _suite_victim_record(pid, born):
    return {
        "event": "guard_tripped",
        "message": f"observed suite RSS victim {pid}",
        "violation": {"rss_kb": 4096, "scope": "process_tree"},
        "shared_sentinel_event": {
            "event": "repo_process_guard_tripped",
            "victim_pgid": pid,
            "violation": {
                "pgid": pid,
                "process_samples": [{"pid": pid, "started_at_ns": born}],
            },
            "termination": {"rss_triggered": True, "attempted": True},
        },
    }


@pytest.mark.parametrize("adapter", ["cpython", "native", "wasm", "llvm", "luau"])
@pytest.mark.parametrize(
    "case",
    [
        "victim",
        "unrelated",
        "reused",
        "success",
        "missing_birth",
        "descendant_unknown_root",
        "descendant",
        "descendant_reused",
        "timeout",
        "infrastructure",
    ],
)
def test_suite_rss_transport_matches_captured_process_instance(
    adapter, case, monkeypatch, tmp_path
):
    from tools import harness_memory_guard, memory_guard
    from tools.memory_guard_core import harness_outcomes

    marker = tmp_path / "trip.json"
    env = {
        "MOLT_EXT_ROOT": str(tmp_path),
        harness_outcomes.SUITE_TRIP_FILE_ENV: str(marker),
    }
    monkeypatch.setattr(molt_diff, "_diff_memory_guard_trip_file", lambda: marker)
    monkeypatch.setattr(molt_diff, "_diff_root", lambda: tmp_path)
    monkeypatch.setattr(molt_diff, "_diff_memory_guard_limits", lambda *_: None)
    child = memory_guard.GuardedChildProcess(
        pid=11,
        pgid=11,
        sid=11,
        command=("fixture",),
        started_at="fixture",
        started_at_ns=None
        if case in {"missing_birth", "descendant_unknown_root"}
        else 1000,
    )
    victim_pid = 22 if case == "unrelated" or case.startswith("descendant") else 11
    born = 2000 if victim_pid == 22 else 1000
    if case in {"reused", "descendant_reused"}:
        born += 1
    rc = 0 if case == "success" else 137
    owned = (
        ((22, memory_guard.ProcessIdentity(2000)),)
        if case.startswith("descendant")
        else ()
    )
    failure = (
        memory_guard.GuardInfrastructureFailure(
            phase="temporary_artifact_custody", details=("existing cleanup failure",)
        )
        if case == "infrastructure"
        else None
    )
    proc = harness_memory_guard.GuardedCompletedProcess(
        ["fixture"],
        124 if case == "timeout" else rc,
        "partial",
        "child stderr",
        elapsed_s=0.1,
        child_process=child,
        child_returncode=rc,
        owned_process_identities=owned,
        infrastructure_failure=failure,
        timed_out=case == "timeout",
        child_stderr="child stderr",
    )

    def launch(*args, **kwargs):
        harness_outcomes.publish_suite_trip(
            marker, _suite_victim_record(victim_pid, born)
        )
        return proc

    if adapter in {"native", "cpython"}:
        monkeypatch.setattr(
            harness_memory_guard.HarnessExecutionContext,
            "from_env",
            lambda *a, **k: SimpleNamespace(run=launch),
        )
        result = molt_diff._run_subprocess(["fixture"], env=env, timeout=5)
    else:
        monkeypatch.setattr(harness_memory_guard, "guarded_completed_process", launch)
        result = compat_backends._guarded_run(
            ["fixture"],
            prefix=f"MOLT_COMPAT_{adapter.upper()}_RUN",
            env=env,
            budget=compat_backends.ItemBudget(
                compat_backends.TimeoutPolicy.from_environment(
                    {"MOLT_TEST_PROCESS_TIMEOUT_SEC": str(5), **env}
                ),
                None,
            ),
            backend="wasm",
            phase="run",
        )
    expected = case in {"victim", "descendant", "descendant_unknown_root"}
    assert result.rss_limit_exceeded is (expected or case == "infrastructure")
    assert result.resource_failure == ("rss_limit_exceeded" if expected else None)
    assert result.child_returncode == rc
    assert result.timed_out is (case == "timeout")
    assert result.stdout == "partial"
    assert result.child_stderr == "child stderr"
    assert result.infrastructure_failure is failure
    assert ("observed suite RSS" in result.stderr) is (
        expected or case == "infrastructure"
    )


def test_guest_retirement_preserves_existing_trip_infrastructure_phase(tmp_path):
    from tools.memory_guard_core.process_custody import GuardInfrastructureFailure

    existing = GuardInfrastructureFailure(
        phase="rss_trip_evidence", details=("marker failed",)
    )
    initial = compat_backends.BackendResult(
        "partial", "diagnostic", 137, infrastructure_failure=existing
    )
    lease = SimpleNamespace(path=tmp_path, retire=lambda **k: "retirement failed")
    result = compat_backends.run_with_guest_outputs(
        [lease], lambda: initial, environment={}, repo_root=_REPO_ROOT
    )
    assert result.infrastructure_failure.phase == "rss_trip_evidence"
    assert result.infrastructure_failure.details == (
        "marker failed",
        "retirement failed",
    )
    assert result.resource_failure is None


@pytest.mark.parametrize("backend", ("cpython", "native", "wasm", "llvm", "luau"))
@pytest.mark.parametrize("build_failed", (False, True))
def test_operator_signal_stops_case_before_semantic_overlay_or_next_backend(
    backend, build_failed, fake_test_file, install_fake_registry, monkeypatch
):
    fake_test_file.write_text(
        "# MOLT_META: expect_fail=molt expect_fail_reason=semantic_gap\nprint(42)\n",
        encoding="utf-8",
    )
    interrupted = compat_backends.BackendResult(
        None if build_failed else "partial",
        "operator interruption",
        143,
        guard_signal=15,
        child_returncode=-9,
    )
    targets = ("native", "wasm", "llvm", "luau")
    outcomes = {name: _outcome("42\n") for name in targets}
    if backend != "cpython":
        outcomes[backend] = interrupted
    registry, native_contexts = install_fake_registry(
        outcomes, cpython=interrupted if backend == "cpython" else ("42\n", "", 0)
    )
    records = []
    monkeypatch.setattr(molt_diff, "_record_diff_result", records.append)
    result = molt_diff.diff_test(str(fake_test_file), targets=targets)
    assert records[0] is result
    assert result["guard_signal"] == 15
    assert result["raw_status"] == result["resolved_status"] == "uncalibrated"
    assert result["reason_tag"] == "guard_interrupted"
    if backend == "cpython":
        assert native_contexts == []
        assert all(not registry[name].contexts for name in targets if name != "native")
        assert result["cpython_guard_signal"] == 15
    else:
        index = targets.index(backend)
        assert [row["backend"] for row in result["backend_rows"]] == list(
            targets[: index + 1]
        )
        assert result["backend_rows"][-1]["guard_signal"] == 15
        assert all(not registry[name].contexts for name in targets[index + 1 :])


@pytest.mark.parametrize("exit_code", (130, 143))
def test_child_exit_matching_signal_status_does_not_cancel(
    fake_test_file, install_fake_registry, exit_code
):
    install_fake_registry(
        {"native": _outcome("", rc=exit_code), "wasm": _outcome("", rc=exit_code)},
        cpython=("", "", exit_code),
    )
    result = molt_diff.diff_test(str(fake_test_file), targets=("native", "wasm"))
    assert result["resolved_status"] == "pass"
    assert result["guard_signal"] is None
    assert len(result["backend_rows"]) == 2


def test_shared_item_budget_caps_successive_guarded_phases(monkeypatch):
    clock = SimpleNamespace(now=0.0)
    monkeypatch.setattr(
        compat_backends, "time", SimpleNamespace(monotonic=lambda: clock.now)
    )
    policy = compat_backends.TimeoutPolicy.from_environment(
        {"MOLT_DIFF_TIMEOUT": "2", "MOLT_DIFF_BUILD_TIMEOUT": "6"}
    )
    budget = compat_backends.ItemBudget.start(policy, ("native",))
    observed = []

    def guarded(command, **kwargs):
        observed.append(kwargs["timeout"])
        clock.now += 4
        return SimpleNamespace(stdout="ok", stderr="", returncode=0, timed_out=False)

    monkeypatch.setattr(
        molt_diff.harness_memory_guard, "guarded_completed_process", guarded
    )
    for _ in range(3):
        result = compat_backends._guarded_run(
            ["fixture"],
            prefix="MOLT_DIFF",
            env={},
            budget=budget,
            backend="native",
            phase="build",
        )
    assert observed == [6, 6, 2]
    assert result.returncode == 0
    refused = compat_backends._guarded_run(
        ["never"],
        prefix="MOLT_DIFF",
        env={},
        budget=budget,
        backend="native",
        phase="run",
    )
    assert refused.timed_out and refused.child_returncode is None
    assert observed == [6, 6, 2]


@pytest.mark.parametrize("remaining", [0.0, 2.0])
def test_native_failure_cannot_replace_the_selected_budget_or_outcome(
    monkeypatch, remaining
):
    clock = SimpleNamespace(now=0.0)
    monkeypatch.setattr(
        compat_backends, "time", SimpleNamespace(monotonic=lambda: clock.now)
    )
    policy = compat_backends.TimeoutPolicy.from_environment(
        {"MOLT_DIFF_TIMEOUT": "2", "MOLT_DIFF_BUILD_TIMEOUT": "6"}
    )
    budget = compat_backends.ItemBudget.start(policy, ("native",))
    context = compat_backends.BackendExecutionContext(
        molt_cli_python=sys.executable,
        target_python=_parse_target_python_version("3.12"),
        build_profile="dev",
        capabilities="",
        environment={},
        budget=budget,
    )
    failure = compat_backends.BackendResult(
        None, "dyld: unknown imports format", 1, build_failed=True
    )
    calls = []

    def native(file, profile, *, execution_context):
        attempt = execution_context.budget
        assert attempt.policy is budget.policy
        assert attempt.deadline == 8.0  # Native's 6+2, excluding CPython's 2.
        calls.append(attempt.limit(policy.phase("native", "build")))
        clock.now = 10.0 - remaining
        return failure

    monkeypatch.setattr(
        molt_diff, "_COMPAT_BACKEND_REGISTRY", compat_backends.build_registry(native)
    )
    result = molt_diff._run_backend_for_diff(
        backend="native", file_path="case.py", context=context
    )
    assert result is failure
    assert calls == [6]
    assert budget.deadline == 10.0


def test_item_deadline_reaches_actual_sleeping_child(monkeypatch, tmp_path):
    # Parent executes this real process control: no compiler or target program.
    monkeypatch.setenv("MOLT_DIFF_ROOT", str(tmp_path / "diff"))
    monkeypatch.setenv("MOLT_DIFF_TMPDIR", str(tmp_path / "scratch"))
    env = dict(os.environ)
    budget = compat_backends.ItemBudget(
        compat_backends.TimeoutPolicy.from_environment({}),
        compat_backends.time.monotonic() + 0.5,
    )
    result = compat_backends._guarded_run(
        [
            sys.executable,
            "-u",
            "-c",
            "import time; print('started'); time.sleep(2); print('late')",
        ],
        prefix="MOLT_COMPAT_WASM_RUN",
        env=env,
        budget=budget,
        backend="wasm",
        phase="run",
    )
    assert result.timed_out
    assert result.infrastructure_failure is None
    assert "started" in (result.stdout or "") and "late" not in (result.stdout or "")


@pytest.mark.parametrize(
    "diagnostic", ["dyld: unknown imports format", "backend daemon connection failed"]
)
def test_native_child_failure_is_reported_once_without_recovery(
    tmp_path, monkeypatch, capsys, diagnostic
):
    source = tmp_path / "case.py"
    source.write_text("print('expected')\n", encoding="utf-8")
    launches = tmp_path / "launches"
    script = "import pathlib,sys\nwith pathlib.Path(sys.argv[1]).open('ab') as sink: sink.write(b'x')\nprint(sys.argv[2],file=sys.stderr)\nraise SystemExit(7)"
    monkeypatch.setenv("MOLT_DIFF_ROOT", str(tmp_path / "evidence"))
    monkeypatch.setenv("MOLT_DIFF_TMPDIR", str(tmp_path / "scratch"))
    # All incident mutations, if an old implementation attempts them, stay in
    # this test's environment and temporary root.
    for key, value in {
        "MOLT_BACKEND_DAEMON": "1",
        "MOLT_DIFF_FORCE_NO_CACHE": "0",
        "MOLT_DIFF_FORCE_REBUILD": "0",
    }.items():
        monkeypatch.setenv(key, value)
    environment = dict(os.environ)
    outcomes = []

    def native(file, profile, *, execution_context, **kwargs):
        result = molt_diff._run_subprocess(
            [sys.executable, "-B", "-c", script, str(launches), diagnostic],
            env=dict(execution_context.environment),
            timeout=30,
        )
        failure = result.as_build_failure(
            detail="injected child failure", fallback="child failed"
        )
        outcomes.append(failure)
        return failure if len(outcomes) == 1 else _outcome("expected\n")

    monkeypatch.setattr(molt_diff, "run_molt", native)
    monkeypatch.setattr(
        molt_diff, "_COMPAT_BACKEND_REGISTRY", compat_backends.build_registry(native)
    )
    monkeypatch.setattr(
        molt_diff, "run_cpython", lambda *a, **k: _outcome("expected\n")
    )
    records = []
    monkeypatch.setattr(molt_diff, "_record_diff_result", records.append)
    result = molt_diff.diff_test(str(source))
    assert launches.read_bytes() == b"x"
    assert len(outcomes) == 1
    assert result["raw_status"] == result["resolved_status"] == "fail"
    assert len(records) == 1
    native_row = records[0]["backend_rows"][0]
    assert native_row["returncode"] == 7
    assert native_row["build_failed"] is True
    assert (
        native_row["stderr_sha256"]
        == hashlib.sha256((diagnostic + "\n").encode()).hexdigest()
    )
    assert outcomes[0].child_stderr == diagnostic + "\n"
    assert diagnostic in capsys.readouterr().out
    assert {
        key: os.environ[key]
        for key in [
            "MOLT_BACKEND_DAEMON",
            "MOLT_DIFF_FORCE_NO_CACHE",
            "MOLT_DIFF_FORCE_REBUILD",
        ]
    } == {
        key: environment[key]
        for key in [
            "MOLT_BACKEND_DAEMON",
            "MOLT_DIFF_FORCE_NO_CACHE",
            "MOLT_DIFF_FORCE_REBUILD",
        ]
    }


# --- Expected failures under exact twins (HF-371 review v4) -----------------

_PINNED_SIGPIPE = (
    "# MOLT_META: expect_fail=molt expect_fail_reason=sigpipe_default_disposition "
    "expect_molt_returncode=-13 expect_molt_stdout_sha256="
    + hashlib.sha256(b"").hexdigest()
    + "\nprint(42)\n"
)


def _twin_record(fake_test_file, install_fake_registry, source, native, jit, cpython):
    fake_test_file.write_text(source, encoding="utf-8")
    install_fake_registry({"native": native, "jit": jit}, cpython=cpython)
    return molt_diff.diff_test(str(fake_test_file), targets=("native", "jit"))


def test_a_twin_divergence_fails_even_on_an_expected_failure(
    fake_test_file, install_fake_registry
):
    # closed_pipe_write_signal.py with the v2 bug back in the JIT tier only:
    # AOT dies by SIGPIPE, JIT prints what CPython prints.
    record = _twin_record(
        fake_test_file,
        install_fake_registry,
        _PINNED_SIGPIPE,
        native=compat_backends.BackendResult("", "", -13),
        jit=compat_backends.BackendResult("BrokenPipeError\n", "", 0),
        cpython=("BrokenPipeError\n", "", 0),
    )
    assert record["resolved_status"] == "fail"
    assert record["reason_tag"] == "cross_backend_divergence"


def test_twins_that_agree_on_an_unpinned_failure_fail(
    fake_test_file, install_fake_registry
):
    # Both tiers fail the same new way (a crash at start), not the pinned way.
    record = _twin_record(
        fake_test_file,
        install_fake_registry,
        _PINNED_SIGPIPE,
        native=compat_backends.BackendResult("", "Segmentation fault", -11),
        jit=compat_backends.BackendResult("", "Segmentation fault", -11),
        cpython=("BrokenPipeError\n", "", 0),
    )
    assert record["resolved_status"] == "fail"
    assert record["reason_tag"] == "pinned_outcome_mismatch"


def test_closed_stderr_start_with_the_keeper_bug_in_the_jit_tier_fails(
    fake_test_file, install_fake_registry
):
    # closed_stderr_start.py if the JIT child refused to start (v3: exit 125).
    source = (
        "# MOLT_META: expect_fail=molt expect_fail_reason=closed_stderr_reused_by_runtime"
        " expect_molt_returncode=0 expect_molt_stdout_sha256="
        + hashlib.sha256(b"rc 0\nchild ran, stderr open\n").hexdigest()
        + "\nprint(42)\n"
    )
    record = _twin_record(
        fake_test_file,
        install_fake_registry,
        source,
        native=compat_backends.BackendResult("rc 0\nchild ran, stderr open\n", "", 0),
        jit=compat_backends.BackendResult("rc 125\n", "", 0),
        cpython=("rc 0\nchild ran, stderr closed\n", "", 0),
    )
    assert record["resolved_status"] == "fail"
    assert record["reason_tag"] == "cross_backend_divergence"


def test_twins_that_end_the_pinned_way_are_the_expected_failure(
    fake_test_file, install_fake_registry
):
    record = _twin_record(
        fake_test_file,
        install_fake_registry,
        _PINNED_SIGPIPE,
        native=compat_backends.BackendResult("", "", -13),
        jit=compat_backends.BackendResult("", "", -13),
        cpython=("BrokenPipeError\n", "", 0),
    )
    assert (record["resolved_status"], record["reason_tag"]) == ("pass", "xfail")


@pytest.mark.parametrize("targets", [("native", "jit"), ("native",)])
def test_an_expected_failure_needs_a_pin_only_under_exact_twins(
    targets, fake_test_file, install_fake_registry
):
    fake_test_file.write_text(
        "# MOLT_META: expect_fail=molt expect_fail_reason=semantic_gap\nprint(42)\n",
        encoding="utf-8",
    )
    failing = compat_backends.BackendResult("", "", -13)
    install_fake_registry(
        {name: failing for name in targets}, cpython=("BrokenPipeError\n", "", 0)
    )
    record = molt_diff.diff_test(str(fake_test_file), targets=targets)
    if len(targets) == 2:
        assert (record["resolved_status"], record["reason_tag"]) == (
            "fail",
            "unpinned_expected_failure",
        )
    else:
        assert (record["resolved_status"], record["reason_tag"]) == ("pass", "xfail")


def test_a_divergence_between_any_backends_cannot_be_xfailed(
    fake_test_file, install_fake_registry
):
    # As a timeout cannot (test_timeout_cannot_be_xfailed_...): two backends
    # that disagree are never the expected Molt-versus-CPython gap.
    fake_test_file.write_text(
        "# MOLT_META: expect_fail=molt expect_fail_reason=semantic_gap\nprint(42)\n",
        encoding="utf-8",
    )
    install_fake_registry(
        {
            "native": compat_backends.BackendResult("1\n", "", 0),
            "wasm": compat_backends.BackendResult("2\n", "", 0),
        },
        cpython=("42\n", "", 0),
    )
    record = molt_diff.diff_test(str(fake_test_file), targets=("native", "wasm"))
    assert record["resolved_status"] == "fail"
    assert record["reason_tag"] == "cross_backend_divergence"


def test_twins_with_the_pinned_status_but_other_output_fail(
    fake_test_file, install_fake_registry
):
    # The same exit status is not the pinned ending when the output differs:
    # here both tiers print before dying by SIGPIPE.
    record = _twin_record(
        fake_test_file,
        install_fake_registry,
        _PINNED_SIGPIPE,
        native=compat_backends.BackendResult("written\n", "", -13),
        jit=compat_backends.BackendResult("written\n", "", -13),
        cpython=("BrokenPipeError\n", "", 0),
    )
    assert record["resolved_status"] == "fail"
    assert record["reason_tag"] == "pinned_outcome_mismatch"
