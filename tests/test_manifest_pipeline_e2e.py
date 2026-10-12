"""End-to-end test: molt build --capability-manifest pipeline.

Tests the full workflow: manifest -> build -> run -> enforcement.
The build proof always enters through the current checkout's CLI module.
"""

from __future__ import annotations

import json
import os
import sys
import tempfile
import time
from pathlib import Path

import pytest

from tests.native_process_guard import run_native_test_process
from tests import process_guard_common

sys.path.insert(0, "src")

PROJECT_ROOT = Path(__file__).parent.parent


def _write_manifest(path: Path) -> None:
    """Write a strict capability manifest for testing."""
    path.write_text(
        "[manifest]\n"
        'version = "2.0"\n'
        "[capabilities]\n"
        'allow = ["time.wall"]\n'
        "[resources]\n"
        'max_memory = "16MB"\n'
        'max_duration = "3s"\n'
        "max_recursion_depth = 100\n"
        "[audit]\n"
        "enabled = true\n"
        'sink = "stderr"\n',
        encoding="utf-8",
    )


# ---------------------------------------------------------------------------
# Test 1: Manifest parses and produces correct env vars
# ---------------------------------------------------------------------------


def test_manifest_parses_and_converts():
    """Manifest loads and produces correct env vars."""
    from molt.capability_manifest import load_manifest

    with tempfile.NamedTemporaryFile(suffix=".toml", mode="w", delete=False) as f:
        _write_manifest(Path(f.name))
        path = f.name
    try:
        m = load_manifest(path)
        env = m.to_env_vars()
        assert env["MOLT_RESOURCE_MAX_MEMORY"] == str(16 * 1024 * 1024), (
            f"expected {16 * 1024 * 1024}, got {env.get('MOLT_RESOURCE_MAX_MEMORY')}"
        )
        assert env["MOLT_RESOURCE_MAX_DURATION_MS"] == "3000", (
            f"expected 3000, got {env.get('MOLT_RESOURCE_MAX_DURATION_MS')}"
        )
        assert env["MOLT_RESOURCE_MAX_RECURSION_DEPTH"] == "100", (
            f"expected 100, got {env.get('MOLT_RESOURCE_MAX_RECURSION_DEPTH')}"
        )
        assert env["MOLT_AUDIT_ENABLED"] == "1"
        assert env["MOLT_AUDIT_SINK"] == "stderr"
        # Capabilities should include time.wall
        assert "time.wall" in env["MOLT_CAPABILITIES"]
    finally:
        os.unlink(path)


# ---------------------------------------------------------------------------
# Test 2: All resource fields map to env vars
# ---------------------------------------------------------------------------


def test_manifest_env_vars_are_complete():
    """All resource fields map to env vars."""
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
    expected_keys = [
        "MOLT_CAPABILITIES",
        "MOLT_RESOURCE_MAX_MEMORY",
        "MOLT_RESOURCE_MAX_DURATION_MS",
        "MOLT_RESOURCE_MAX_ALLOCATIONS",
        "MOLT_RESOURCE_MAX_RECURSION_DEPTH",
    ]
    for key in expected_keys:
        assert key in env, f"missing env var: {key}"


# ---------------------------------------------------------------------------
# Test 3: Effective capabilities respect deny list
# ---------------------------------------------------------------------------


def test_manifest_deny_removes_capabilities():
    """Denied capabilities are excluded from effective set."""
    from molt.capability_manifest import CapabilityManifest

    m = CapabilityManifest(
        allow=["net", "time.wall"],
        deny=["websocket.connect"],
    )
    effective = m.effective_capabilities()
    assert "time.wall" in effective
    assert "net" not in effective
    assert "net.connect" in effective
    # The profile expands to exact operation grants; deny removes one grant
    # without preserving the old broad bypass token.
    assert "websocket.connect" not in effective


# ---------------------------------------------------------------------------
# Test 6: Env var round-trip (duration and memory edge cases)
# ---------------------------------------------------------------------------


def test_env_var_round_trip_edge_cases():
    """Verify env var values for edge-case resource limits."""
    from molt.capability_manifest import (
        CapabilityManifest,
        ManifestError,
        ResourceLimits,
    )

    m = CapabilityManifest(resources=ResourceLimits(max_duration=0.0, max_memory=0))
    with pytest.raises(ManifestError, match="max_memory must be positive"):
        m.to_env_vars()

    # Fractional seconds
    m2 = CapabilityManifest(resources=ResourceLimits(max_duration=1.5))
    env2 = m2.to_env_vars()
    assert env2["MOLT_RESOURCE_MAX_DURATION_MS"] == "1500"


# ---------------------------------------------------------------------------
# Test 7: Audit disabled produces no audit env vars
# ---------------------------------------------------------------------------


def test_audit_disabled_no_env_vars():
    """When audit is disabled, no MOLT_AUDIT_* env vars are emitted."""
    from molt.capability_manifest import CapabilityManifest, AuditConfig

    m = CapabilityManifest(audit=AuditConfig(enabled=False))
    env = m.to_env_vars()
    assert "MOLT_AUDIT_ENABLED" not in env
    assert "MOLT_AUDIT_SINK" not in env


# ---------------------------------------------------------------------------
# Test 8: Manifest with strict limits parses size/duration strings
# ---------------------------------------------------------------------------


def test_parse_size_and_duration_from_manifest():
    """Size and duration strings in TOML are parsed to numeric values."""
    from molt.capability_manifest import load_manifest

    toml_content = (
        "[manifest]\n"
        'version = "2.0"\n'
        "[resources]\n"
        'max_memory = "256KB"\n'
        'max_duration = "500ms"\n'
    )
    with tempfile.NamedTemporaryFile(suffix=".toml", mode="w", delete=False) as f:
        f.write(toml_content)
        path = f.name
    try:
        m = load_manifest(path)
        assert m.resources.max_memory == 256 * 1024
        assert m.resources.max_duration == 0.5
        env = m.to_env_vars()
        assert env["MOLT_RESOURCE_MAX_MEMORY"] == str(256 * 1024)
        assert env["MOLT_RESOURCE_MAX_DURATION_MS"] == "500"
    finally:
        os.unlink(path)


# ---------------------------------------------------------------------------
# Test 9: Full molt build pipeline (skipped if molt unavailable)
# ---------------------------------------------------------------------------


@pytest.mark.slow
def test_molt_build_with_manifest():
    """Build and execute through the current checkout's complete CLI graph."""
    with process_guard_common.guarded_temporary_directory(
        prefix="molt-manifest-build-"
    ) as tmpdir:
        manifest = tmpdir / "test.capabilities.toml"
        _write_manifest(manifest)

        src = tmpdir / "hello.py"
        src.write_text('print("hello from molt")\n', encoding="utf-8")

        result = run_native_test_process(
            [
                sys.executable,
                "-m",
                "molt.cli",
                "build",
                "--profile",
                "dev",
                "--capability-manifest",
                str(manifest),
                str(src),
                "--out-dir",
                str(tmpdir),
                "--json",
            ],
            capture_output=True,
            text=True,
            timeout=600,
            cwd=str(PROJECT_ROOT),
        )
        assert result.returncode == 0, (
            f"stdout:\n{result.stdout}\n\nstderr:\n{result.stderr}"
        )
        payload = json.loads(result.stdout)
        output = Path(payload["data"]["output"])
        assert output.is_file()

        from molt.capability_manifest import load_manifest

        loaded = load_manifest(str(manifest))
        env = {**os.environ, **loaded.to_env_vars()}
        run_result = run_native_test_process(
            [str(output)],
            capture_output=True,
            text=True,
            timeout=10,
            env=env,
        )
        assert run_result.returncode == 0, run_result.stderr
        assert run_result.stdout.strip() == "hello from molt"


@pytest.mark.slow
def test_native_warm_code_cache_keeps_current_capability_policy() -> None:
    """Shared code must not carry the previous final binary's authority."""
    with process_guard_common.guarded_temporary_directory(
        prefix="molt-native-cache-policy-"
    ) as tmpdir:
        payload_path = tmpdir / "payload.txt"
        payload_path.write_text("cache-policy-payload\n", encoding="utf-8")
        source = tmpdir / "read_payload.py"
        source.write_text(
            "try:\n"
            f"    with open({str(payload_path)!r}, 'r') as stream:\n"
            "        print(stream.read(), end='')\n"
            "except PermissionError:\n"
            "    print('PermissionError')\n",
            encoding="utf-8",
        )
        source_bytes = source.read_bytes()
        manifest = tmpdir / "policy.toml"
        cache_dir = tmpdir / "code-cache"
        output_dir = tmpdir / "output"
        empty_path = tmpdir / "no-python-on-path"
        empty_path.mkdir()
        observations = []
        output = None
        for index, allowed in enumerate((True, False, True)):
            manifest.write_text(
                '[manifest]\nversion = "2.0"\n[capabilities]\n'
                + ('allow = ["fs.read"]\n' if allowed else "allow = []\n"),
                encoding="utf-8",
            )
            started = time.monotonic()
            built = run_native_test_process(
                [
                    sys.executable,
                    "-m",
                    "molt.cli",
                    "build",
                    "--profile",
                    "dev",
                    "--cache",
                    "--cache-dir",
                    str(cache_dir),
                    "--capability-manifest",
                    str(manifest),
                    str(source),
                    "--out-dir",
                    str(output_dir),
                    "--json",
                ],
                cwd=PROJECT_ROOT,
                timeout=1200,
            )
            elapsed = time.monotonic() - started
            assert built.returncode == 0, (
                f"build {index}: stdout:\n{built.stdout}\nstderr:\n{built.stderr}"
            )
            data = json.loads(built.stdout)["data"]
            current_output = Path(data["output"])
            assert current_output.is_file()
            if output is None:
                output = current_output
            assert current_output == output
            assert source.read_bytes() == source_bytes
            cache = data["cache"]
            assert cache["enabled"] is True
            assert isinstance(cache["key"], str) and cache["key"]
            if observations:
                assert cache["key"] == observations[0]["cache_key"]
                assert cache["hit"] is True, data
            assert data["capabilities"] == (["fs.read"] if allowed else [])
            observation = {
                "allow_fs_read": allowed,
                "elapsed_seconds": elapsed,
                "cache_key": cache["key"],
                "cache_hit": cache["hit"],
                "policy_digest": data["capability_policy_digest"],
            }
            observations.append(observation)
            print(
                "native-cache-policy-build " + json.dumps(observation, sort_keys=True)
            )

            # The emitted stub, not ambient grants or Python, owns this run.
            # This is a checkout-built executable control, not installed-package
            # or general standalone-distribution acceptance.
            environment = dict(os.environ)
            environment.pop("PYTHONPATH", None)
            environment["PATH"] = str(empty_path)
            environment["MOLT_CAPABILITY_TIER"] = "none" if allowed else "full"
            environment["MOLT_CAPABILITIES"] = "" if allowed else "fs.read"
            executed = run_native_test_process(
                [str(current_output)],
                cwd=tmpdir,
                env=environment,
                timeout=30,
            )
            assert executed.returncode == 0, (
                f"run {index}: stdout:\n{executed.stdout}\nstderr:\n{executed.stderr}"
            )
            assert executed.stdout == (
                "cache-policy-payload\n" if allowed else "PermissionError\n"
            )
            assert executed.stderr == ""
            assert payload_path.read_text(encoding="utf-8") == "cache-policy-payload\n"
        assert observations[0]["policy_digest"] != observations[1]["policy_digest"]
        assert observations[0]["policy_digest"] == observations[2]["policy_digest"]


# ---------------------------------------------------------------------------
# Runner
# ---------------------------------------------------------------------------

if __name__ == "__main__":
    tests = [
        v for k, v in sorted(globals().items()) if k.startswith("test_") and callable(v)
    ]
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
    sys.exit(1 if failed else 0)
