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
import hashlib
import subprocess
import sys
from pathlib import Path
from types import SimpleNamespace

import pytest
from molt.target_python import TargetPythonVersion

_REPO_ROOT = Path(__file__).resolve().parents[1]
for _p in (str(_REPO_ROOT), str(_REPO_ROOT / "tests"), str(_REPO_ROOT / "src")):
    if _p not in sys.path:
        sys.path.insert(0, _p)

import molt_diff  # noqa: E402
from tools.compat import backends as compat_backends  # noqa: E402


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
        timeout_default=60.0,
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
        timeout_default=600.0,
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

    def availability(self) -> compat_backends.BackendAvailability:
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
            target_python="3.14",
            build_profile="dev",
            capabilities="",
            environment={},
        )
    with pytest.raises(ValueError, match="must be canonical"):
        compat_backends.BackendExecutionContext(
            target_python=TargetPythonVersion(3, 14, 1),
            build_profile="dev",
            capabilities="",
            environment={},
        )


@pytest.mark.parametrize("target_python", ("3.13", "3.14"))
@pytest.mark.parametrize("backend", ("wasm", "llvm", "luau"))
@pytest.mark.parametrize("stdlib_profile", (None, "micro", "full"))
def test_cross_backend_build_command_binds_target_python(
    monkeypatch: pytest.MonkeyPatch,
    tmp_path: Path,
    target_python: str,
    backend: str,
    stdlib_profile: str | None,
) -> None:
    monkeypatch.setattr(compat_backends, "_molt_cli_python", lambda: "python")
    context = compat_backends.BackendExecutionContext(
        target_python=TargetPythonVersion(3, int(target_python.split(".")[1]), 0),
        build_profile="release",
        capabilities="fs.read",
        environment={
            "MOLT_CAPABILITY_TIER": "none",
            "MOLT_DIFF_STDLIB_PROFILE": stdlib_profile or "",
        },
    )

    command = compat_backends._build_cmd(
        "case.py",
        backend,
        tmp_path,
        context,
    )

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
            target_python=TargetPythonVersion(3, 12, 0),
            build_profile="dev",
            capabilities="",
            environment={"MOLT_DIFF_STDLIB_PROFILE": "wide"},
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
        ["fixture"], prefix="MOLT_COMPAT_WASM_RUN", env={}, timeout_default=60
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


# ---------------------------------------------------------------------------
# Full diff_test multi-backend path with a fake registry (no real build).
# ---------------------------------------------------------------------------


@pytest.fixture
def fake_test_file(tmp_path) -> Path:
    f = tmp_path / "prog.py"
    f.write_text("print(42)\n")
    return f


@pytest.fixture
def install_fake_registry(monkeypatch):
    """Install a fake backend registry into molt_diff and stub run_cpython.

    Returns a function that takes a mapping {backend: BackendResult}, installs it
    as the registry, and stubs CPython to a chosen oracle output.
    """

    def _install(backend_results: dict, cpython=("42\n", "", 0)):
        registry = {
            name: _FakeAdapter(name, result) for name, result in backend_results.items()
        }
        native_contexts: list[compat_backends.BackendExecutionContext] = []
        # native still flows through run_molt -> stub run_molt to return the
        # native scripted result so even native is in-memory here.
        native_result = backend_results.get("native")

        def _fake_run_molt(file_path, build_profile, **kwargs):
            assert native_result is not None, "native result must be provided"
            context = kwargs.get("execution_context")
            assert isinstance(context, compat_backends.BackendExecutionContext)
            assert context.build_profile == build_profile
            native_contexts.append(context)
            return native_result

        monkeypatch.setattr(molt_diff, "run_molt", _fake_run_molt)
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
        fake_test_file.write_text("# MOLT_META: stdlib_profile=full\nprint(42)\n")
    targets = ("native", "wasm", "llvm", "luau")
    registry, native_contexts = install_fake_registry(
        {target: compat_backends.BackendResult("42\n", "", 0) for target in targets}
    )
    records: list[dict[str, object]] = []
    monkeypatch.setattr(molt_diff, "_record_diff_result", records.append)
    assert molt_diff.diff_test(str(fake_test_file), targets=targets) == "pass"
    context = native_contexts[0]
    assert context.stdlib_profile == "full"
    for target in targets[1:]:
        assert registry[target].contexts == [context]
    backend_rows = records[0]["backend_rows"]
    assert isinstance(backend_rows, list)
    assert len(backend_rows) == len(targets)
    assert all(row["stdlib_profile"] == "full" for row in backend_rows)


def test_native_and_wasm_receive_one_explicit_untrusted_test_context(
    fake_test_file: Path,
    install_fake_registry,
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    fake_test_file.write_text(
        "# MOLT_ENV: MOLT_CAPABILITIES=net.listen,net.outbound\nprint(42)\n",
        encoding="utf-8",
    )
    monkeypatch.setenv("MOLT_CAPABILITY_TIER", "full")
    monkeypatch.setenv("MOLT_CAPABILITIES", "poison.inherited")
    monkeypatch.delenv("MOLT_DIFF_TRUSTED", raising=False)
    registry, native_contexts = install_fake_registry(
        {
            "native": compat_backends.BackendResult("42\n", "", 0),
            "wasm": compat_backends.BackendResult("42\n", "", 0),
        }
    )

    status = molt_diff.diff_test(
        str(fake_test_file),
        targets=("native", "wasm"),
        target_python=f"{sys.version_info.major}.{sys.version_info.minor}",
    )

    assert status == "pass"
    wasm_contexts = registry["wasm"].contexts
    assert len(native_contexts) == len(wasm_contexts) == 1
    assert native_contexts[0] == wasm_contexts[0]
    assert native_contexts[0].target_python.short == (
        f"{sys.version_info.major}.{sys.version_info.minor}"
    )
    assert native_contexts[0].environment["MOLT_CAPABILITY_TIER"] == "none"
    assert native_contexts[0].capabilities == "net.listen,net.outbound"
    assert "poison.inherited" not in native_contexts[0].capabilities


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
    status = molt_diff.diff_test(str(fake_test_file), targets=("native", "wasm"))
    assert status == "pass"


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

    status = molt_diff.diff_test(str(fake_test_file), targets=("native", "wasm"))

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
    native_only = molt_diff.diff_test(str(fake_test_file), targets=("native",))
    assert native_only == "pass"
    # Multi-backend: RED.
    status = molt_diff.diff_test(str(fake_test_file), targets=("native", "wasm"))
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
    status = molt_diff.diff_test(str(fake_test_file), targets=("native", "wasm"))
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
    status = molt_diff.diff_test(str(fake_test_file), targets=("native", "wasm"))
    assert status == "fail"


def test_fault_injection_inert_when_unset() -> None:
    base = compat_backends.BackendResult("42\n", "", 0)
    out = compat_backends._apply_fault_injection("wasm", base)
    assert out.stdout == base.stdout  # no env -> no perturbation


def test_uncalibrated_when_no_backend_available(fake_test_file, monkeypatch) -> None:
    # A backend whose toolchain is unavailable is a LOUD uncalibrated, never a
    # silent pass. With only an unavailable backend requested, the test resolves
    # to "uncalibrated".
    class _Unavailable:
        name = "luau"

        def availability(self):
            return compat_backends.BackendAvailability(
                available=False, reason="lune not on PATH"
            )

        def build_and_run(self, *a, **k):  # pragma: no cover - never called
            raise AssertionError("unavailable backend must not run")

    monkeypatch.setattr(molt_diff, "_COMPAT_BACKEND_REGISTRY", {"luau": _Unavailable()})
    monkeypatch.setattr(molt_diff, "run_cpython", lambda *a, **k: _outcome("42\n"))
    status = molt_diff.diff_test(str(fake_test_file), targets=("luau",))
    assert status == "uncalibrated"


@pytest.mark.parametrize("prefix", _COMPAT_GUARD_PHASES)
@pytest.mark.parametrize("expired_exception", (False, True))
def test_timeout_preserves_diagnostic_and_is_never_oom(
    prefix, expired_exception, monkeypatch
) -> None:
    from tools import harness_memory_guard

    def finish(command, **kwargs):
        if expired_exception:
            raise subprocess.TimeoutExpired(
                command, kwargs["timeout"], output=b"partial", stderr=b"killed"
            )
        return SimpleNamespace(
            stdout="partial", stderr="killed", returncode=137, timed_out=True
        )

    monkeypatch.setattr(harness_memory_guard, "guarded_completed_process", finish)
    result = compat_backends._guarded_run(
        ["noop"], prefix=prefix, env={}, timeout_default=60.0
    )
    assert result.stdout == "partial"
    assert "killed" in result.stderr
    assert "timeout after 60.0s" in result.stderr
    assert result.returncode == 124 and result.timed_out
    assert not molt_diff._should_retry_oom(
        result.returncode, "out of memory: " + result.stderr
    )


@pytest.mark.parametrize("backend", ("wasm", "llvm", "luau"))
@pytest.mark.parametrize("phase", ("build", "run"))
@pytest.mark.parametrize("failure_kind", ("timeout", "infrastructure"))
def test_all_adapters_preserve_phase_failure(
    backend, phase, failure_kind, tmp_path, monkeypatch
):
    monkeypatch.setattr(compat_backends, "_scratch_dir", lambda *_: tmp_path)
    monkeypatch.setattr(compat_backends, "_molt_cli_python", lambda: "python")
    context = compat_backends.BackendExecutionContext(
        target_python=TargetPythonVersion(3, 12, 0),
        build_profile="dev",
        capabilities="",
        environment={},
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
        target_python=TargetPythonVersion(3, 12, 0),
        build_profile="dev",
        capabilities="",
        environment={},
    )
    adapter = compat_backends.NativeAdapter(
        lambda *a, **k: compat_backends.BackendResult(
            None, "deadline", 124, build_failed=True, timed_out=True
        )
    )
    result = adapter.build_and_run("case.py", context=context)
    assert result.timed_out and result.build_failed


def test_native_build_timeout_keeps_partial_compiler_diagnostics():
    result = compat_backends.BackendResult.from_timeout(
        subprocess.TimeoutExpired(
            ["compiler"], 3, output=b"phase detail", stderr=b"error detail"
        ),
        build_failed=True,
    )
    assert result.stdout is None and result.build_failed and result.timed_out
    assert "phase detail" in result.stderr and "error detail" in result.stderr


@pytest.mark.parametrize("backend", ("native", "wasm", "llvm", "luau"))
def test_timeout_cannot_be_xfailed_even_with_other_backend_divergence(
    backend, fake_test_file, install_fake_registry, monkeypatch
):
    fake_test_file.write_text(
        "# MOLT_META: expect_fail=molt expect_fail_reason=semantic_gap\nprint(42)\n"
    )
    targets = ("native", "wasm", "llvm", "luau")
    outcomes = {name: _outcome(name) for name in targets}
    outcomes[backend] = compat_backends.BackendResult(
        None, "killed after timeout", 124, build_failed=True, timed_out=True
    )
    install_fake_registry(outcomes)
    records = []
    monkeypatch.setattr(molt_diff, "_record_diff_result", records.append)
    assert molt_diff.diff_test(str(fake_test_file), targets=targets) == "fail"
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
                None, diagnostic, 137, build_failed=True
            ),
        }
    )
    records = []
    monkeypatch.setattr(molt_diff, "_record_diff_result", records.append)
    assert molt_diff.diff_test(str(fake_test_file), targets=("native", "wasm")) == "oom"
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
        "# MOLT_META: expect_fail=molt expect_fail_reason=semantic_gap\nprint(42)\n"
    )
    registry, _ = install_fake_registry(
        {"native": _outcome(native_output), "luau": _outcome("")}
    )
    monkeypatch.setattr(
        registry["luau"],
        "availability",
        lambda: compat_backends.BackendAvailability(False, "runner missing"),
    )
    records = []
    monkeypatch.setattr(molt_diff, "_record_diff_result", records.append)
    assert (
        molt_diff.diff_test(str(fake_test_file), targets=("native", "luau"))
        == "uncalibrated"
    )
    assert records[0]["backend_rows"][1]["detail"] == "runner missing"
    assert records[0]["backend_rows"][1]["returncode"] is None


def test_cpython_timeout_cannot_become_semantic_parity(
    fake_test_file, install_fake_registry, monkeypatch
):
    fake_test_file.write_text(
        "# MOLT_META: expect_fail=molt expect_fail_reason=semantic_gap\nprint(42)\n"
    )
    registry, native_contexts = install_fake_registry(
        {"native": _outcome("", rc=124)},
        cpython=compat_backends.BackendResult(
            "", "oracle deadline", 124, timed_out=True
        ),
    )
    records = []
    monkeypatch.setattr(molt_diff, "_record_diff_result", records.append)
    assert molt_diff.diff_test(str(fake_test_file)) == "fail"
    assert records[0]["reason_tag"] == "timeout"
    assert native_contexts == []


def test_intentional_exit_124_is_not_a_timeout(fake_test_file, install_fake_registry):
    install_fake_registry(
        {"native": _outcome("", rc=124), "wasm": _outcome("", rc=124)},
        cpython=("", "", 124),
    )
    assert (
        molt_diff.diff_test(str(fake_test_file), targets=("native", "wasm")) == "pass"
    )


@pytest.mark.parametrize("backend", ("native", "wasm", "llvm", "luau"))
@pytest.mark.parametrize("build_failed", (False, True))
def test_infrastructure_failure_is_not_semantic_or_oom_evidence(
    backend, build_failed, fake_test_file, install_fake_registry, monkeypatch, capsys
):
    fake_test_file.write_text(
        "# MOLT_META: expect_fail=molt expect_fail_reason=semantic_gap\nprint(42)\n"
    )
    targets = ("native", "wasm", "llvm", "luau")
    outcomes = {name: _outcome(name) for name in targets}
    failure = _infrastructure_outcome(child_returncode=137, build_failed=build_failed)
    outcomes[backend] = failure
    install_fake_registry(outcomes)
    monkeypatch.setenv("MOLT_COMPAT_FAULT_INJECT", backend)
    records = []
    monkeypatch.setattr(molt_diff, "_record_diff_result", records.append)
    assert molt_diff.diff_test(str(fake_test_file), targets=targets) == "uncalibrated"
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


def test_cpython_infrastructure_failure_cannot_be_semantic_parity(
    fake_test_file, install_fake_registry, monkeypatch
):
    oracle = _infrastructure_outcome()
    _, contexts = install_fake_registry(
        {"native": _outcome("partial", rc=125)}, cpython=oracle
    )
    records = []
    monkeypatch.setattr(molt_diff, "_record_diff_result", records.append)
    assert molt_diff.diff_test(str(fake_test_file)) == "uncalibrated"
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
    assert molt_diff.diff_test(str(fake_test_file)) == "pass"


@pytest.mark.parametrize("entry", ["initial", "after-dyld", "after-daemon"])
def test_native_infrastructure_failure_never_triggers_another_retry_or_quarantine(
    monkeypatch, entry
):
    failure = _infrastructure_outcome(build_failed=True)
    results = [failure] if entry == "initial" else [_outcome(None, rc=1), failure]
    calls = []

    def run(*args, **kwargs):
        calls.append(kwargs)
        return results.pop(0)

    monkeypatch.setattr(molt_diff, "run_molt", run)
    monkeypatch.setattr(
        molt_diff, "_diff_retry_dyld_default", lambda: entry == "after-dyld"
    )
    monkeypatch.setattr(molt_diff, "_is_dyld_unknown_imports", lambda _: True)
    monkeypatch.setattr(molt_diff, "_is_backend_daemon_build_error", lambda _: True)
    monkeypatch.setattr(molt_diff, "_mark_dyld_guard", lambda _: None)
    for hook in (
        "_diff_disable_daemon_on_dyld",
        "_diff_retry_isolated_default",
        "_diff_force_rebuild_on_dyld",
    ):
        monkeypatch.setattr(
            molt_diff,
            hook,
            lambda: pytest.fail("infrastructure cannot authorize retry/quarantine"),
        )
    context = compat_backends.BackendExecutionContext(
        target_python=TargetPythonVersion(3, 12, 0),
        build_profile="dev",
        capabilities="",
        environment={},
    )
    assert molt_diff._run_native_backend("fixture.py", context) is failure
    assert len(calls) == (1 if entry == "initial" else 2)
    assert results == []
