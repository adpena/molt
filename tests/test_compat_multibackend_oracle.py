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
from dataclasses import replace
import hashlib
import os
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
from tools.compat import diff_output_layout  # noqa: E402


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
        target_python=TargetPythonVersion(3, 12, 0),
        build_profile="dev",
        capabilities="",
        environment={},
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
    f.write_text("print(42)\n", encoding="utf-8")
    return f


@pytest.fixture
def cpython_oracle(monkeypatch) -> TargetPythonVersion:
    """Declare the stubbed CPython oracle's identity instead of probing one.

    run_cpython is stubbed, so no live interpreter answers for the oracle; the
    real probe is a guarded subprocess whose result, failures included, is
    cached for the whole process. The declared minor is supported but differs
    from the interpreter running pytest, so a target derived from the host
    rather than the oracle cannot pass. Target admission itself stays real.
    """
    oracle = TargetPythonVersion(3, 12 if sys.version_info[:2] == (3, 13) else 13, 0)
    command = molt_diff._resolve_python_command(sys.executable)

    def version(probed: tuple[str, ...]) -> tuple[int, int]:
        assert probed == command, f"probed {probed}, not the oracle {command}"
        return oracle.feature_version

    def sys_env(probed: tuple[str, ...]) -> dict[str, str]:
        assert probed == command, f"probed {probed}, not the oracle {command}"
        return {
            "MOLT_PYTHON_VERSION": oracle.short,
            "MOLT_SYS_VERSION_INFO": (
                f"{oracle.major},{oracle.minor},{oracle.micro},"
                f"{oracle.release},{oracle.serial}"
            ),
        }

    monkeypatch.setattr(molt_diff, "_python_command_version", version)
    monkeypatch.setattr(molt_diff, "_molt_sys_env_for_python_command", sys_env)
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
        fake_test_file.write_text(
            "# MOLT_META: stdlib_profile=full\nprint(42)\n", encoding="utf-8"
        )
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
    cpython_oracle: TargetPythonVersion,
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
        target_python=cpython_oracle.short,
    )

    assert status == "pass"
    wasm_contexts = registry["wasm"].contexts
    assert len(native_contexts) == len(wasm_contexts) == 1
    assert native_contexts[0] == wasm_contexts[0]
    assert native_contexts[0].target_python == cpython_oracle
    assert native_contexts[0].environment["MOLT_PYTHON_VERSION"] == (
        cpython_oracle.short
    )
    assert native_contexts[0].environment["MOLT_CAPABILITY_TIER"] == "none"
    assert native_contexts[0].capabilities == "net.listen,net.outbound"
    assert "poison.inherited" not in native_contexts[0].capabilities


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
        )
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


@pytest.mark.usefixtures("cpython_oracle")
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
    assert replace(result, diagnostic_stderr="MemoryError").resource_failure is None


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
                None, diagnostic, 137, build_failed=True, rss_limit_exceeded=True
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
        "# MOLT_META: expect_fail=molt expect_fail_reason=semantic_gap\nprint(42)\n",
        encoding="utf-8",
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
    assert molt_diff.diff_test(str(fake_test_file), targets=targets) == "uncalibrated"
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
@pytest.mark.parametrize(
    "kind", ["infrastructure", "rss", "allocation", "interrupted", "timeout"]
)
def test_native_infrastructure_failure_never_triggers_another_retry_or_quarantine(
    monkeypatch, entry, kind
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
    }[kind]
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
            lambda: pytest.fail(
                "terminal execution evidence cannot authorize retry/quarantine"
            ),
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
        ["fixture"], prefix=prefix, env={}, timeout_default=5
    )
    for converted in [
        actual,
        actual.as_build_failure(detail="build failed", fallback="failed"),
    ]:
        assert converted.rss_limit_exceeded is measured
        assert converted.child_returncode == result.child_returncode
        assert converted.diagnostic_stderr == "runtime crashed\n"
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
    assert molt_diff.diff_test(str(fake_test_file), targets=(backend,)) == "fail"
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
    assert molt_diff.diff_test(str(fake_test_file), targets=("native",)) == (
        "oom" if measured else "fail"
    )
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
    env = {harness_outcomes.SUITE_TRIP_FILE_ENV: str(marker)}
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
        rc,
        "partial",
        "child stderr",
        elapsed_s=0.1,
        child_process=child,
        child_returncode=rc,
        owned_process_identities=owned,
        infrastructure_failure=failure,
        timed_out=case == "timeout",
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
        if case == "timeout":
            with pytest.raises(subprocess.TimeoutExpired):
                molt_diff._run_subprocess(["fixture"], env=env, timeout=5)
            return
        result = molt_diff._run_subprocess(["fixture"], env=env, timeout=5)
    else:
        monkeypatch.setattr(harness_memory_guard, "guarded_completed_process", launch)
        result = compat_backends._guarded_run(
            ["fixture"], prefix="MOLT_" + adapter.upper(), env=env, timeout_default=5
        )
    expected = case in {"victim", "descendant", "descendant_unknown_root"}
    assert result.rss_limit_exceeded is (expected or case == "infrastructure")
    assert result.resource_failure == ("rss_limit_exceeded" if expected else None)
    assert result.child_returncode == rc
    assert result.stdout == "partial"
    assert result.diagnostic_stderr == "child stderr"
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
