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
def test_cross_backend_build_command_binds_target_python(
    monkeypatch: pytest.MonkeyPatch,
    tmp_path: Path,
    target_python: str,
    backend: str,
) -> None:
    monkeypatch.setattr(compat_backends, "_molt_cli_python", lambda: "python")
    context = compat_backends.BackendExecutionContext(
        target_python=TargetPythonVersion(3, int(target_python.split(".")[1]), 0),
        build_profile="release",
        capabilities="fs.read",
        environment={"MOLT_TRUSTED": "0"},
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


# ---------------------------------------------------------------------------
# Direct tests of the divergence helper
# ---------------------------------------------------------------------------


def _outcome(stdout, rc=0, stderr=""):
    return molt_diff._BackendOutcome(stdout=stdout, stderr=stderr, returncode=rc)


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
            return (
                native_result.stdout,
                native_result.stderr,
                native_result.returncode,
            )

        monkeypatch.setattr(molt_diff, "run_molt", _fake_run_molt)
        monkeypatch.setattr(molt_diff, "_COMPAT_BACKEND_REGISTRY", registry)
        monkeypatch.setattr(molt_diff, "run_cpython", lambda *a, **k: cpython)
        return registry, native_contexts

    return _install


def test_native_and_wasm_receive_one_explicit_untrusted_test_context(
    fake_test_file: Path,
    install_fake_registry,
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    fake_test_file.write_text(
        "# MOLT_ENV: MOLT_CAPABILITIES=net.listen,net.outbound\nprint(42)\n",
        encoding="utf-8",
    )
    monkeypatch.setenv("MOLT_TRUSTED", "1")
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
    assert native_contexts[0].environment["MOLT_TRUSTED"] == "0"
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
    monkeypatch.setattr(molt_diff, "run_cpython", lambda *a, **k: ("42\n", "", 0))
    status = molt_diff.diff_test(str(fake_test_file), targets=("luau",))
    assert status == "uncalibrated"
