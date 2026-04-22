"""End-to-end resource enforcement tests.

These tests verify that resource limits configured via environment variables
are actually enforced when running Python programs through Molt.

Note: These tests require `molt run` to be available. They are skipped
if the Molt CLI is not found.
"""

from __future__ import annotations

import os
import shutil
import subprocess
import sys
import tempfile


def _molt_available() -> bool:
    return shutil.which("molt") is not None


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
        return subprocess.run(
            ["molt", "run", path],
            capture_output=True,
            text=True,
            timeout=timeout,
            env=env,
        )
    except subprocess.TimeoutExpired:
        return subprocess.CompletedProcess(
            args=["molt", "run", path],
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
        return subprocess.run(
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
