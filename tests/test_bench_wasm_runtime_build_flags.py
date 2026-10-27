from __future__ import annotations

import contextlib

import hashlib
import io
import json
import subprocess
from molt.temporary_artifacts import OwnedTemporaryDirectory
import time
from pathlib import Path

import pytest

import tools.bench_wasm as bench_wasm
from tests.process_guard_common import install_module_view
from molt.cli import atomic_io, wasm_link_args
from tests.runtime_profile_fixtures import (
    process_profile_payload,
    profile_epoch_payload,
)


@pytest.mark.parametrize("outcome", ["timeout", "lock", "compile", "manifest", "pass"])
def test_wasm_benchmark_preserves_one_admitted_build_attempt(
    tmp_path, monkeypatch, outcome
) -> None:
    env = {
        "CARGO_TARGET_DIR": str(tmp_path / "target"),
        "MOLT_BUILD_STATE_DIR": str(tmp_path / "control"),
        "MOLT_MIDEND_MAX_ROUNDS": "12",
    }
    original_env = dict(env)
    output = tmp_path / "output.wasm"
    output.write_bytes(b"preserved failure evidence")
    calls = []
    diagnostics = []
    ticks = iter((10.0, 12.5))
    install_module_view(
        monkeypatch,
        "time",
        time,
        bench_wasm,
        perf_counter=lambda: next(ticks),
    )
    monkeypatch.setattr(
        bench_wasm, "molt_args_for_benchmark", lambda _script: ["--wrong-selection"]
    )
    monkeypatch.setattr(bench_wasm, "_parse_env_float", lambda *a, **k: 90.0)
    monkeypatch.setattr(
        bench_wasm, "_write_build_timeout_diag", lambda **k: diagnostics.append(k)
    )
    monkeypatch.setattr(
        bench_wasm,
        "_linked_wasm_output",
        lambda _path: None if outcome == "manifest" else output,
    )

    def run(cmd, **kwargs):
        calls.append((cmd, dict(kwargs["env"])))
        return bench_wasm._RunResult(
            returncode=1 if outcome in {"timeout", "lock", "compile"} else 0,
            timed_out=outcome == "timeout",
            stderr=(
                "Timed out after 1.0s waiting for the build lock (MOLT_BUILD_LOCK_TIMEOUT)"
                if outcome == "lock"
                else ""
            ),
        )

    monkeypatch.setattr(bench_wasm, "_run_cmd", run)
    result = bench_wasm._build_wasm_output(
        ["python"],
        env,
        output,
        "bench.py",
        molt_args=["--profile", "dev"],
        tty=False,
        log=None,
    )
    assert len(calls) == 1
    assert calls[0][0][-3:] == ["--profile", "dev", "bench.py"]
    assert "--wrong-selection" not in calls[0][0]
    assert calls[0][1] == env == original_env
    assert output.read_bytes() == b"preserved failure evidence"
    assert result == (2.5 if outcome == "pass" else None)
    assert bool(diagnostics) == (outcome == "timeout")
    if outcome == "pass":
        assert bench_wasm._LAST_BUILD_FAILURE_DETAIL is None
    else:
        assert bench_wasm._LAST_BUILD_FAILURE_DETAIL


@contextlib.contextmanager
def _admitted_workloads(scripts):
    from tools.bench_evidence import admit_benchmark_workload

    with contextlib.ExitStack() as custody:
        yield {
            script: custody.enter_context(
                admit_benchmark_workload(
                    Path(script),
                    project_root=Path(bench_wasm.__file__).resolve().parents[1],
                    run_args=[],
                    molt_args=bench_wasm.molt_args_for_benchmark(script),
                )
            )
            for script in scripts
        }


def test_prepare_wasm_binary_does_not_retry_failed_build(
    monkeypatch,
) -> None:
    calls: list[tuple[str, dict[str, str]]] = []
    prunes: list[dict[str, str]] = []
    monkeypatch.setattr(bench_wasm, "_base_env", lambda: {"MOLT_WASM_TABLE_BASE": "0"})
    monkeypatch.setattr(
        bench_wasm, "_prune_backend_daemons", lambda env: prunes.append(dict(env))
    )
    monkeypatch.setattr(bench_wasm, "_python_cmd", lambda: ["python"])

    def failed_build(
        _python_cmd,
        env,
        _output,
        script,
        *,
        molt_args,
        tty,
        log,
        limits,
        use_molt_build_cache,
    ):
        del tty, log, limits, use_molt_build_cache
        assert molt_args == []
        calls.append((script, dict(env)))
        bench_wasm._LAST_BUILD_FAILURE_DETAIL = "build_timeout timeout_s=90.0"
        return None

    monkeypatch.setattr(bench_wasm, "_build_wasm_output", failed_build)
    result = bench_wasm.prepare_wasm_binary(
        "case.py", molt_args=[], tty=False, log=None, keep_temp=False, limits=object()
    )
    assert result is None
    assert len(calls) == len(prunes) == 1
    assert bench_wasm._LAST_BUILD_FAILURE_DETAIL == "build_timeout timeout_s=90.0"


@pytest.mark.parametrize("reloc", (False, True))
@pytest.mark.parametrize(
    "profile,gpu", (("micro", False), ("micro", True), ("full", False))
)
@pytest.mark.parametrize(
    "outcome", ("pass", "timeout", "failure", "malformed", "missing")
)
def test_runtime_build_consumes_one_canonical_generation_result(
    monkeypatch, tmp_path: Path, reloc: bool, profile: str, gpu: bool, outcome: str
) -> None:
    from molt import llvm_toolchain
    from molt.cli import wasm_link_inputs

    monkeypatch.setattr(
        llvm_toolchain,
        "apply_provisioned_wasm_toolchain",
        lambda *a, **k: pytest.fail("benchmark repeated SDK selection"),
    )
    monkeypatch.setattr(
        wasm_link_inputs,
        "resolve_wasi_c_abi_plan",
        lambda *a, **k: pytest.fail("benchmark repeated member capture"),
    )
    target_root = tmp_path / "target"
    monkeypatch.setattr(bench_wasm, "_cargo_target_root", lambda: target_root)
    monkeypatch.setenv("MOLT_STDLIB_PROFILE", profile)
    monkeypatch.setenv("MOLT_WASM_RUNTIME_GPU_PRIMITIVES", "1" if gpu else "0")
    output = tmp_path / "requested.wasm"
    output.write_bytes(b"preserve old output")
    kind = "reloc" if reloc else "shared"
    selected = tmp_path / "selected-generation" / (kind + ".wasm")
    generation = selected.parent / "generation.json"
    calls = []

    def run(command, **kwargs):
        calls.append((command, kwargs))
        selected.parent.mkdir()
        generation.write_text("fixture compiler-owned receipt", encoding="utf-8")
        if outcome != "missing":
            selected.write_bytes(b"\0asm\x01\0\0\0")
        stdout = (
            "not json"
            if outcome == "malformed"
            else json.dumps(
                {
                    "status": "ok",
                    "artifacts": {kind: str(selected), "generation": str(generation)},
                }
            )
        )
        return bench_wasm._RunResult(
            returncode=1 if outcome == "failure" else 0,
            timed_out=outcome == "timeout",
            stdout=stdout,
        )

    monkeypatch.setattr(bench_wasm, "_run_cmd", run)
    assert bench_wasm.build_runtime_wasm(
        reloc=reloc, output=output, tty=False, log=None
    ) is (outcome == "pass")
    assert len(calls) == 1
    command, kwargs = calls[0]
    assert command[:4] == [
        bench_wasm.sys.executable,
        "-m",
        "molt.cli",
        "internal-runtime-wasm-build",
    ]
    assert command[command.index("--kind") + 1] == kind
    assert command[command.index("--stdlib-profile") + 1] == profile
    assert "--json" in command and "--cargo-timeout" in command
    assert "--features" not in command and "--" not in command
    assert ("--runtime-feature" in command) is gpu
    if gpu:
        assert command[command.index("--runtime-feature") + 1] == "molt_gpu_primitives"
    assert kwargs["env"]["MOLT_WASM_RUNTIME_GPU_PRIMITIVES"] == ("1" if gpu else "0")
    assert kwargs["capture"] is True
    assert kwargs["timeout_s"] > 0
    assert output.read_bytes() == (
        b"\0asm\x01\0\0\0" if outcome == "pass" else b"preserve old output"
    )


def test_wasm_link_response_is_content_addressed_stable_and_windows_safe(
    monkeypatch,
    tmp_path: Path,
) -> None:
    project_root = tmp_path / "repo with spaces"
    project_root.mkdir()
    monkeypatch.setenv("MOLT_BUILD_STATE_DIR", str(project_root / "state with spaces"))
    link_args = [
        "--import-memory",
        "--export-if-defined=molt_beta",
        "--export-if-defined=molt_alpha",
    ]
    link_flags = tuple(
        token for arg in link_args for token in ("-C", f"link-arg={arg}")
    )
    writes: list[Path] = []
    atomic_write = atomic_io._atomic_write_bytes

    def record_atomic_write(path: Path, payload: bytes) -> None:
        writes.append(path)
        atomic_write(path, payload)

    monkeypatch.setattr(atomic_io, "_atomic_write_bytes", record_atomic_write)
    first = wasm_link_args.wasm_link_args_response_file(
        project_root,
        label="runtime shared",
        link_flags=link_flags,
    )
    second = wasm_link_args.wasm_link_args_response_file(
        project_root,
        label="runtime shared",
        link_flags=link_flags,
    )

    assert first is not None
    assert second == first
    assert writes == [first]
    payload = "".join(f"{arg}\n" for arg in link_args).encode("utf-8")
    digest = hashlib.sha256(payload).hexdigest()
    assert first.name == f"runtime_shared.{digest}.rsp"
    assert first.read_bytes() == payload
    assert " " in str(first)
    rustc_args = ["-C", f"link-arg=@{first}"]
    rendered = subprocess.list2cmdline(rustc_args)
    assert f'"link-arg=@{first}"' in rendered


@pytest.mark.parametrize(
    "argument",
    ["--export=has space", "--export=has\nnewline", "--export=has\0nul", "@nested.rsp"],
)
def test_wasm_link_response_rejects_ambiguous_entries(
    tmp_path: Path, argument: str
) -> None:
    with pytest.raises(ValueError):
        wasm_link_args.write_wasm_link_args_response_file(
            tmp_path,
            label="unsafe",
            link_args=[argument],
        )


def test_failed_wasm_run_has_null_time_and_samples(monkeypatch, tmp_path: Path) -> None:
    script = tmp_path / "bench_fail.py"
    script.write_text("print(1)\n", encoding="utf-8")
    temp_dir = OwnedTemporaryDirectory()
    wasm = bench_wasm.WasmBinary(
        run_env={},
        temp_dir=temp_dir,
        build_s=0.25,
        size_kb=12.5,
        linked_used=True,
        import_count_total=None,
        import_count_functions=None,
        import_count_tables=None,
    )

    monkeypatch.setattr(bench_wasm, "prepare_wasm_binary", lambda *args, **kwargs: wasm)
    monkeypatch.setattr(
        bench_wasm,
        "collect_samples",
        lambda *args, **kwargs: (
            [],
            False,
            bench_wasm._SampleResult(
                elapsed_s=None,
                returncode=1,
                error="runtime failed",
                error_class="runtime_error",
            ),
            [],
        ),
    )

    with _admitted_workloads([str(script)]) as workloads:
        results = bench_wasm.bench_results(
            [str(script)],
            samples=1,
            warmup=0,
            super_run=True,
            runner_cmd=["node"],
            runner_name="node",
            control_runner_cmd=None,
            control_runner_name=None,
            tty=False,
            log=None,
            keep_temp=False,
            workloads=workloads,
        )

    entry = results[str(script)]
    assert entry["workload"]["entry"] == str(script)
    assert entry["workload"]["run_args"] == []
    assert entry["molt_wasm_ok"] is False
    assert entry["molt_wasm_time_s"] is None
    assert entry["molt_wasm_samples_s"] == []
    assert entry["molt_wasm_failure_class"] == "runtime_error"


def test_measure_wasm_run_uses_guard_child_elapsed(monkeypatch) -> None:
    limits = bench_wasm.harness_memory_guard.limits_from_env("MOLT_BENCH", {})
    calls: list[dict[str, object]] = []

    def fake_run_cmd(*args, **kwargs):
        calls.append(kwargs)
        return bench_wasm._RunResult(
            returncode=0,
            stdout="",
            stderr="",
            elapsed_s=0.045,
        )

    monkeypatch.setattr(
        bench_wasm,
        "_run_cmd",
        fake_run_cmd,
    )

    result = bench_wasm.measure_wasm_run(
        {},
        ["node", "wasm/run_wasm.js"],
        runner_name="node",
        log=None,
        limits=limits,
    )

    assert result.elapsed_s == 0.045
    assert result.error is None
    assert calls[0]["limits"] is limits


def test_wasm_run_cmd_routes_tty_timeout_through_guard(monkeypatch) -> None:
    limits = bench_wasm.harness_memory_guard.HarnessMemoryLimits(
        enabled=False,
        max_process_rss_gb=1.0,
        max_total_rss_gb=1.0,
        max_global_rss_gb=1.0,
        poll_interval=0.1,
    )
    calls: list[dict[str, object]] = []

    def fake_guard(command, **kwargs):
        calls.append({"command": command, **kwargs})
        completed = subprocess.CompletedProcess(
            command,
            bench_wasm.harness_memory_guard.memory_guard.TIMEOUT_RETURN_CODE,
            "stdout",
            "TERM_CLEANUP\n",
        )
        completed.elapsed_s = 0.1
        return completed

    monkeypatch.setattr(
        bench_wasm.harness_memory_guard,
        "guarded_completed_process",
        fake_guard,
    )

    log = io.StringIO()
    result = bench_wasm._run_cmd(
        ["node", "runner.js"],
        env={},
        capture=False,
        tty=True,
        log=log,
        timeout_s=0.1,
        limits=limits,
    )

    assert (
        result.returncode
        == bench_wasm.harness_memory_guard.memory_guard.TIMEOUT_RETURN_CODE
    )
    assert result.timed_out is True
    assert result.elapsed_s == 0.1
    assert "TERM_CLEANUP" in result.stderr
    assert "TERM_CLEANUP" in log.getvalue()
    assert calls[0]["command"] == ["node", "runner.js"]
    assert calls[0]["capture_output"] is True
    assert calls[0]["timeout"] == 0.1
    assert calls[0]["limits"] is limits


def test_partial_wasm_sample_failure_has_null_time(monkeypatch, tmp_path: Path) -> None:
    script = tmp_path / "bench_partial.py"
    script.write_text("print(1)\n", encoding="utf-8")
    temp_dir = OwnedTemporaryDirectory()
    wasm = bench_wasm.WasmBinary(
        run_env={},
        temp_dir=temp_dir,
        build_s=0.25,
        size_kb=12.5,
        linked_used=True,
        import_count_total=None,
        import_count_functions=None,
        import_count_tables=None,
    )

    monkeypatch.setattr(bench_wasm, "prepare_wasm_binary", lambda *args, **kwargs: wasm)
    monkeypatch.setattr(
        bench_wasm,
        "collect_samples",
        lambda *args, **kwargs: (
            [0.01],
            False,
            bench_wasm._SampleResult(
                elapsed_s=None,
                returncode=1,
                error="second sample failed",
                error_class="runtime_error",
            ),
            [],
        ),
    )

    with _admitted_workloads([str(script)]) as workloads:
        results = bench_wasm.bench_results(
            [str(script)],
            samples=2,
            warmup=0,
            super_run=False,
            runner_cmd=["node"],
            runner_name="node",
            control_runner_cmd=None,
            control_runner_name=None,
            tty=False,
            log=None,
            keep_temp=False,
            workloads=workloads,
        )

    entry = results[str(script)]
    assert entry["workload"]["entry"] == str(script)
    assert entry["workload"]["run_args"] == []
    assert entry["molt_wasm_ok"] is False
    assert entry["molt_wasm_time_s"] is None
    assert entry["molt_wasm_samples_s"] == [0.01]


def test_collect_samples_rejects_partial_sample_failure(monkeypatch) -> None:
    temp_dir = OwnedTemporaryDirectory()
    wasm = bench_wasm.WasmBinary(
        run_env={},
        temp_dir=temp_dir,
        build_s=0.25,
        size_kb=12.5,
        linked_used=True,
        import_count_total=None,
        import_count_functions=None,
        import_count_tables=None,
    )
    results = iter(
        [
            bench_wasm._SampleResult(None, 1, "failed", "runtime_error"),
            bench_wasm._SampleResult(0.01, 0, None, None),
        ]
    )
    monkeypatch.setattr(
        bench_wasm, "measure_wasm_run", lambda *args, **kwargs: next(results)
    )

    samples, ok, failure, profiles = bench_wasm.collect_samples(
        wasm,
        samples=2,
        warmup=0,
        runner_cmd=["node"],
        runner_name="node",
        log=None,
    )

    assert samples == [0.01]
    assert ok is False
    assert failure is not None
    assert failure.error_class == "runtime_error"
    assert profiles == [{"sample_index": 1, "profile": None, "epochs": []}]


def test_wasm_profile_parsers_require_current_schemas_and_preserve_epochs() -> None:
    process = process_profile_payload()
    process["profile"]["alloc_count"] = 1
    process["profile"]["dealloc_count"] = 1
    epochs = []
    for generation, label in ((1, "cache_hits"), (2, "weakref_calls")):
        epoch = profile_epoch_payload()
        epoch["generation"] = generation
        epoch["label"] = label
        epochs.append(epoch)
    log = "\n".join(
        [
            'molt_profile_json {"profile":{"alloc_count":99}}',
            "molt_profile_json " + json.dumps(process),
            *("molt_profile_epoch_json " + json.dumps(epoch) for epoch in epochs),
        ]
    )

    assert bench_wasm._extract_profile_json(log) == process
    assert bench_wasm._extract_profile_epoch_json(log) == epochs
    assert (
        bench_wasm._extract_profile_json(
            'molt_profile_json {"profile":{"alloc_count":99}}'
        )
        is None
    )


def test_zero_duration_wasm_run_is_invalid_sample(monkeypatch) -> None:
    install_module_view(
        monkeypatch,
        "time",
        time,
        bench_wasm,
        perf_counter=lambda: 10.0,
    )
    monkeypatch.setattr(
        bench_wasm,
        "_run_cmd",
        lambda *args, **kwargs: bench_wasm._RunResult(returncode=0),
    )

    result = bench_wasm.measure_wasm_run({}, ["node"], runner_name="node", log=None)

    assert result.elapsed_s is None
    assert result.returncode == 0
    assert result.error_class == "invalid_timing"


def test_runtime_target_is_checkout_scoped_unless_a_session_is_pinned(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    """bench_wasm built cold into target/sessions/bench-wasm-<pid> (HF-144)."""
    ext_root = tmp_path / "ext-root"
    monkeypatch.delenv("CARGO_TARGET_DIR", raising=False)
    monkeypatch.delenv("MOLT_SESSION_ID", raising=False)
    monkeypatch.delenv("MOLT_SESSION_ID_GENERATED", raising=False)
    monkeypatch.setenv("MOLT_EXT_ROOT", str(ext_root))
    from molt.dx import checkout_component

    target = (
        ext_root.resolve()
        / "target"
        / "checkouts"
        / checkout_component(bench_wasm._repo_root())
    )
    assert bench_wasm._cargo_target_root() == target

    monkeypatch.setenv("MOLT_SESSION_ID", "lane-a")
    assert bench_wasm._cargo_target_root() == (target / "sessions" / "lane-a")


def test_wasm_producer_threads_admitted_molt_args_to_actual_build_argv(
    tmp_path, monkeypatch
):
    from tools.bench_evidence import admit_benchmark_workload
    from tests.wasm_execution_manifest import (
        EMPTY_WASM_MODULE,
        write_wasm_execution_manifest,
    )

    script = tmp_path / "program.py"
    script.write_text("print(1)\n", encoding="utf-8")
    commands = []
    monkeypatch.setattr(
        bench_wasm, "molt_args_for_benchmark", lambda _: ["--wrong-selection"]
    )
    monkeypatch.setattr(bench_wasm, "_base_env", lambda: {"MOLT_WASM_TABLE_BASE": "0"})
    monkeypatch.setattr(bench_wasm, "_prune_backend_daemons", lambda _: None)
    monkeypatch.setattr(bench_wasm, "_python_cmd", lambda: ["selected-python"])
    monkeypatch.setattr(
        bench_wasm.harness_memory_guard,
        "repo_process_sentinel",
        lambda **_: contextlib.nullcontext(),
    )

    def run(command, **kwargs):
        commands.append(command)
        out = Path(command[command.index("--out-dir") + 1]) / "output.wasm"
        out.write_bytes(EMPTY_WASM_MODULE)
        linked = out.with_name("output_linked.wasm")
        linked.write_bytes(EMPTY_WASM_MODULE)
        write_wasm_execution_manifest(out.parent, linked=linked)
        return bench_wasm._RunResult(0)

    monkeypatch.setattr(bench_wasm, "_run_cmd", run)
    monkeypatch.setattr(
        bench_wasm, "collect_samples", lambda *a, **k: ([0.1], True, None, [])
    )
    with admit_benchmark_workload(
        script, project_root=tmp_path, run_args=[], molt_args=["--profile", "dev"]
    ) as admitted:
        data = bench_wasm.bench_results(
            [str(script)],
            1,
            0,
            False,
            workloads={str(script): admitted},
            runner_cmd=["node"],
            runner_name="node",
            control_runner_cmd=None,
            control_runner_name=None,
            tty=False,
            log=None,
            keep_temp=False,
        )
    assert len(commands) == 1
    assert commands[0][:4] == ["selected-python", "-m", "molt.cli", "build"]
    assert commands[0][-3:] == ["--profile", "dev", str(script)]
    assert "--wrong-selection" not in commands[0]
    assert data[str(script)]["workload"]["molt_args"] == ["--profile", "dev"]


def test_wasm_producer_preserves_source_drift_when_scratch_cleanup_fails(
    tmp_path, monkeypatch
):
    from tools.bench_evidence import admit_benchmark_workload

    script = tmp_path / "program.py"
    script.write_text("print(1)\n", encoding="utf-8")
    owner = OwnedTemporaryDirectory()
    wasm = bench_wasm.WasmBinary({}, owner, 0.1, 1.0, True, None, None, None)
    original_cleanup = type(owner).cleanup
    monkeypatch.setattr(
        bench_wasm.harness_memory_guard,
        "repo_process_sentinel",
        lambda **_: contextlib.nullcontext(),
    )

    def prepare(*args, **kwargs):
        script.write_text("print(2)\n", encoding="utf-8")
        return wasm

    def cleanup(self):
        if self is owner:
            raise OSError("independent scratch cleanup refusal")
        original_cleanup(self)

    monkeypatch.setattr(bench_wasm, "prepare_wasm_binary", prepare)
    monkeypatch.setattr(
        bench_wasm,
        "collect_samples",
        lambda *a, **k: pytest.fail("drift reached sample"),
    )
    try:
        with monkeypatch.context() as scoped:
            scoped.setattr(type(owner), "cleanup", cleanup)
            with pytest.raises(ValueError, match="changed") as caught:
                with admit_benchmark_workload(
                    script, project_root=tmp_path, run_args=[], molt_args=[]
                ) as admitted:
                    bench_wasm.bench_results(
                        [str(script)],
                        1,
                        0,
                        False,
                        workloads={str(script): admitted},
                        runner_cmd=["node"],
                        runner_name="node",
                        control_runner_cmd=None,
                        control_runner_name=None,
                        tty=False,
                        log=None,
                        keep_temp=False,
                    )
            assert caught.value.benchmark_temporary_directories == (owner,)
            assert any(
                "independent scratch cleanup refusal" in note
                for note in caught.value.__notes__
            )
            assert Path(owner.name).is_dir()
    finally:
        original_cleanup(owner)


def test_wasm_producer_refuses_unsupported_admitted_guest_args_before_build(
    tmp_path, monkeypatch
):
    from tools.bench_evidence import admit_benchmark_workload

    script = tmp_path / "program.py"
    script.write_text("print(1)\n", encoding="utf-8")
    monkeypatch.setattr(
        bench_wasm.harness_memory_guard,
        "repo_process_sentinel",
        lambda **_: contextlib.nullcontext(),
    )
    monkeypatch.setattr(
        bench_wasm,
        "prepare_wasm_binary",
        lambda *a, **k: pytest.fail("unsupported argv reached build"),
    )
    with admit_benchmark_workload(
        script, project_root=tmp_path, run_args=["different"], molt_args=[]
    ) as admitted:
        with pytest.raises(ValueError, match="does not support guest run arguments"):
            bench_wasm.bench_results(
                [str(script)],
                1,
                0,
                False,
                workloads={str(script): admitted},
                runner_cmd=["node"],
                runner_name="node",
                control_runner_cmd=None,
                control_runner_name=None,
                tty=False,
                log=None,
                keep_temp=False,
            )
