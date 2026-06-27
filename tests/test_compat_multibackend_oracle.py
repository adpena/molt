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

import sys
from pathlib import Path

import pytest

_REPO_ROOT = Path(__file__).resolve().parents[1]
for _p in (str(_REPO_ROOT), str(_REPO_ROOT / "tests"), str(_REPO_ROOT / "src")):
    if _p not in sys.path:
        sys.path.insert(0, _p)

import molt_diff  # noqa: E402
from tools.compat import backends as compat_backends  # noqa: E402


# ---------------------------------------------------------------------------
# A fake in-memory backend adapter: returns a scripted result, no real build.
# ---------------------------------------------------------------------------


class _FakeAdapter:
    def __init__(self, name: str, result: compat_backends.BackendResult) -> None:
        self.name = name
        self._result = result

    def availability(self) -> compat_backends.BackendAvailability:
        return compat_backends.BackendAvailability(available=True)

    def build_and_run(
        self, file_path, build_profile, *, extra_env, capabilities
    ) -> compat_backends.BackendResult:
        return self._result


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
            name: _FakeAdapter(name, result)
            for name, result in backend_results.items()
        }
        # native still flows through run_molt -> stub run_molt to return the
        # native scripted result so even native is in-memory here.
        native_result = backend_results.get("native")

        def _fake_run_molt(file_path, build_profile, **kwargs):
            assert native_result is not None, "native result must be provided"
            return (
                native_result.stdout,
                native_result.stderr,
                native_result.returncode,
            )

        monkeypatch.setattr(molt_diff, "run_molt", _fake_run_molt)
        monkeypatch.setattr(molt_diff, "_COMPAT_BACKEND_REGISTRY", registry)
        monkeypatch.setattr(
            molt_diff, "run_cpython", lambda *a, **k: cpython
        )

    return _install


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
    status = molt_diff.diff_test(
        str(fake_test_file), targets=("native", "wasm")
    )
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
    status = molt_diff.diff_test(
        str(fake_test_file), targets=("native", "wasm")
    )
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
    status = molt_diff.diff_test(
        str(fake_test_file), targets=("native", "wasm")
    )
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
    status = molt_diff.diff_test(
        str(fake_test_file), targets=("native", "wasm")
    )
    assert status == "fail"


def test_fault_injection_inert_when_unset() -> None:
    base = compat_backends.BackendResult("42\n", "", 0)
    out = compat_backends._apply_fault_injection("wasm", base)
    assert out.stdout == base.stdout  # no env -> no perturbation


def test_uncalibrated_when_no_backend_available(
    fake_test_file, monkeypatch
) -> None:
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

    monkeypatch.setattr(
        molt_diff, "_COMPAT_BACKEND_REGISTRY", {"luau": _Unavailable()}
    )
    monkeypatch.setattr(molt_diff, "run_cpython", lambda *a, **k: ("42\n", "", 0))
    status = molt_diff.diff_test(str(fake_test_file), targets=("luau",))
    assert status == "uncalibrated"
