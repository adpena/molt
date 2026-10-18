"""End-to-end resource enforcement tests.

These tests verify that resource limits configured via environment variables
are actually enforced when running Python programs through Molt.

The compiled cases run this checkout's CLI through the test interpreter,
never a `molt` that happens to be first on PATH.
"""

from __future__ import annotations

import os
import subprocess
import sys
import tempfile

import pytest

from tests.native_process_guard import run_native_test_process


# The checkout under test, not an installed `molt` script.
_MOLT_CLI = (sys.executable, "-m", "molt.cli")


def _run_molt(
    source: str, env_overrides: dict[str, str] | None = None, timeout: int = 10
) -> subprocess.CompletedProcess:
    """Write source to a temp file and run via molt run."""
    with tempfile.NamedTemporaryFile(mode="w", suffix=".py", delete=False) as f:
        f.write(source)
        path = f.name
    try:
        env = os.environ.copy()
        if env_overrides:
            env.update(env_overrides)
        return run_native_test_process(
            [*_MOLT_CLI, "run", path],
            capture_output=True,
            text=True,
            timeout=timeout,
            env=env,
        )
    except subprocess.TimeoutExpired:
        return subprocess.CompletedProcess(
            args=[*_MOLT_CLI, "run", path],
            returncode=-1,
            stdout="",
            stderr="TIMEOUT",
        )
    finally:
        os.unlink(path)


def _run_python(source: str, timeout: int = 10) -> subprocess.CompletedProcess:
    """Run source through CPython for comparison."""
    with tempfile.NamedTemporaryFile(mode="w", suffix=".py", delete=False) as f:
        f.write(source)
        path = f.name
    try:
        return run_native_test_process(
            [sys.executable, path],
            capture_output=True,
            text=True,
            timeout=timeout,
        )
    except subprocess.TimeoutExpired:
        return subprocess.CompletedProcess(
            args=[sys.executable, path],
            returncode=-1,
            stdout="",
            stderr="TIMEOUT",
        )
    finally:
        os.unlink(path)


# --- Tests that work against CPython (DoS guards in ops_arith.rs) ---


def test_dos_pow_guard_cpython():
    """2 ** 10_000_000 should be guarded or fail safely."""
    result = _run_python("x = 2 ** 10_000_000\nprint(len(str(x)))")
    # CPython >= 3.11 raises ValueError on str() of huge ints due to
    # sys.int_max_str_digits.  Molt's guard in ops_arith.rs rejects the
    # pow itself with MemoryError.  Either is acceptable -- the key point
    # is that the process does NOT hang or OOM-kill.
    assert (
        result.returncode == 0
        or "ValueError" in result.stderr
        or "MemoryError" in result.stderr
    )


def test_dos_lshift_guard_cpython():
    """1 << 100_000_000 should be guarded or fail safely."""
    result = _run_python("x = 1 << 100_000_000\nprint(len(str(x)))")
    # Same situation as pow: CPython may raise ValueError on str
    # conversion, Molt rejects the shift with MemoryError.
    assert (
        result.returncode == 0
        or "ValueError" in result.stderr
        or "MemoryError" in result.stderr
    )


def test_recursion_limit_cpython():
    """Deep recursion should raise RecursionError."""
    result = _run_python("""
def f(n):
    return f(n+1)
try:
    f(0)
except RecursionError:
    print("caught")
""")
    assert result.returncode == 0
    assert "caught" in result.stdout


# --- Tests that verify env var propagation ---


def test_resource_env_vars_set():
    """Verify CapabilityManifest.to_env_vars produces correct vars."""
    sys.path.insert(0, "src")
    from molt.capability_manifest import CapabilityManifest, ResourceLimits

    m = CapabilityManifest(
        resources=ResourceLimits(
            max_memory=1048576,
            max_duration=5.0,
            max_allocations=1000,
            max_recursion_depth=50,
        )
    )
    env = m.to_env_vars()
    assert env["MOLT_RESOURCE_MAX_MEMORY"] == "1048576"
    assert env["MOLT_RESOURCE_MAX_DURATION_MS"] == "5000"
    assert env["MOLT_RESOURCE_MAX_ALLOCATIONS"] == "1000"
    assert env["MOLT_RESOURCE_MAX_RECURSION_DEPTH"] == "50"


def test_resource_env_vars_absent_when_no_limits():
    """Default manifest should not produce resource env vars."""
    sys.path.insert(0, "src")
    from molt.capability_manifest import CapabilityManifest

    m = CapabilityManifest()
    env = m.to_env_vars()
    assert "MOLT_RESOURCE_MAX_MEMORY" not in env
    assert "MOLT_RESOURCE_MAX_DURATION_MS" not in env


@pytest.mark.skipif(os.name != "posix", reason="extended spawn uses POSIX sessions")
def test_child_resource_policy_survives_real_environment_mutation():
    """Inspect actual child environments through both shipped spawn entrypoints."""
    keys = [
        "MOLT_RESOURCE_MAX_MEMORY",
        "MOLT_RESOURCE_MAX_DURATION_MS",
        "MOLT_RESOURCE_MAX_ALLOCATIONS",
        "MOLT_RESOURCE_MAX_RECURSION_DEPTH",
        "MOLT_RESOURCE_MAX_OPERATION_RESULT",
        "MOLT_RESOURCE_MAX_POW_RESULT",
        "MOLT_RESOURCE_MAX_REPEAT_RESULT",
        "MOLT_RESOURCE_MAX_SHIFT_RESULT",
        "MOLT_RESOURCE_MAX_STRING_RESULT",
    ]
    initial = dict(
        zip(
            keys,
            ["64M", "30000", "1000000", "500", "4096", "2048", "2048", "2048", "2048"],
        )
    )
    initial.update(MOLT_CAPABILITY_TIER="full", MOLT_CHILD_RLIMIT_GB="1")
    child = (
        "import os; print(','.join(os.environ.get(k, 'missing') for k in "
        + repr(keys)
        + ")); print(os.environ.get('APPLICATION_VALUE', 'missing'))"
    )
    source = f"""
import os
import subprocess
keys = {keys!r}
for key in keys:
    os.putenv(key, "999999999")
    os.environ[key] = "999999999"
os.putenv("MOLT_CHILD_RLIMIT_GB", "0")
os.environ["APPLICATION_VALUE"] = "dynamic"
for extended in (False, True):
    for requested in (None, {{"MOLT_RESOURCE_MAX_MEMORY": "32M", "MOLT_RESOURCE_MAX_OPERATION_RESULT": "1024", "APPLICATION_VALUE": "replacement"}}):
        result = subprocess.run([{sys.executable!r}, "-c", {child!r}], env=requested, capture_output=True, text=True, start_new_session=extended)
        assert result.returncode == 0, result.stderr
        print(result.stdout, end="")
"""
    result = _run_molt(source, initial, timeout=180)
    assert result.returncode == 0, result.stderr
    inherited = "67108864,30000,1000000,500,4096,2048,2048,2048,2048"
    tighter = "33554432,30000,1000000,500,1024,1024,1024,1024,1024"
    assert (
        result.stdout.splitlines() == [inherited, "dynamic", tighter, "replacement"] * 2
    )


def test_runtime_feedback_destination_cannot_be_enabled_by_environment_write(tmp_path):
    """A guest may change its environment without gaining a file output sink."""
    canary = tmp_path / "unauthorized-feedback.json"
    source = f"""
import os
os.putenv("MOLT_RUNTIME_FEEDBACK", "1")
os.putenv("MOLT_RUNTIME_FEEDBACK_FILE", {str(canary)!r})
os.environ["MOLT_RUNTIME_FEEDBACK"] = "1"
os.environ["MOLT_RUNTIME_FEEDBACK_FILE"] = {str(canary)!r}
print("completed")
"""
    result = _run_molt(
        source,
        {
            "MOLT_CAPABILITY_TIER": "none",
            "MOLT_CAPABILITIES": "env.write",
            "MOLT_PROFILE": "1",
            "MOLT_RUNTIME_FEEDBACK": "0",
            "MOLT_RUNTIME_FEEDBACK_FILE": str(canary),
        },
        timeout=180,
    )
    assert result.returncode == 0, result.stderr
    assert result.stdout.strip() == "completed"
    assert not canary.exists(), (
        "late environment writes must not create a feedback output destination"
    )


if __name__ == "__main__":
    tests = [v for k, v in sorted(globals().items()) if k.startswith("test_")]
    passed = failed = 0
    for t in tests:
        try:
            t()
            passed += 1
            print(f"  PASS  {t.__name__}")
        except Exception as e:
            failed += 1
            print(f"  FAIL  {t.__name__}: {e}")
    print(f"\n{passed}/{passed + failed} passed")
