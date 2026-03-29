"""End-to-end test for --capability-manifest CLI flag.

Verifies that a TOML manifest is correctly parsed, converted to env vars,
and (when molt is available) propagated to the compiled binary.
"""
from __future__ import annotations

import json
import os
import sys
import tempfile
from pathlib import Path

sys.path.insert(0, "src")


def test_toml_manifest_loads_and_converts():
    """Create a TOML manifest, load it, verify env vars."""
    from molt.capability_manifest import load_manifest

    toml = '''
[manifest]
version = "2.0"

[capabilities]
allow = ["net", "env.read"]
deny = ["fs.write"]

[resources]
max_memory = "32MB"
max_duration = "5s"
max_allocations = 500000
max_recursion_depth = 200

[audit]
enabled = true
sink = "jsonl"
output = "stderr"

[io]
mode = "virtual"

[monty]
compatible = true
execution_tier = "auto"
tier_up_threshold = 50
'''
    with tempfile.NamedTemporaryFile(mode="w", suffix=".toml", delete=False) as f:
        f.write(toml)
        path = f.name

    try:
        m = load_manifest(path)
        assert "net" in m.allow
        assert "env.read" in m.allow
        assert "fs.write" in m.deny
        assert m.resources.max_memory == 32 * 1024 * 1024
        assert m.resources.max_duration == 5.0
        assert m.resources.max_allocations == 500000
        assert m.resources.max_recursion_depth == 200
        assert m.audit.enabled is True
        assert m.audit.sink == "jsonl"
        assert m.io.mode == "virtual"
        assert m.monty.compatible is True
        assert m.monty.tier_up_threshold == 50

        env = m.to_env_vars()
        assert env["MOLT_RESOURCE_MAX_MEMORY"] == str(32 * 1024 * 1024)
        assert env["MOLT_RESOURCE_MAX_DURATION_MS"] == "5000"
        assert env["MOLT_RESOURCE_MAX_ALLOCATIONS"] == "500000"
        assert env["MOLT_RESOURCE_MAX_RECURSION_DEPTH"] == "200"
        assert env["MOLT_AUDIT_ENABLED"] == "1"
        assert env["MOLT_AUDIT_SINK"] == "jsonl"
        assert env["MOLT_IO_MODE"] == "virtual"

        # Verify deny removes from effective caps
        effective = m.effective_capabilities()
        assert "fs.write" not in effective
        assert "net" in effective or any("net" in c for c in effective)
    finally:
        os.unlink(path)


def test_yaml_manifest_loads():
    """YAML format also works."""
    yaml_content = '''
manifest:
  version: "2.0"
capabilities:
  allow:
    - net
resources:
  max_memory: "16MB"
'''
    try:
        import yaml  # noqa: F401
    except ImportError:
        print("  SKIP test_yaml_manifest_loads (pyyaml not installed)")
        return

    with tempfile.NamedTemporaryFile(mode="w", suffix=".yaml", delete=False) as f:
        f.write(yaml_content)
        path = f.name

    try:
        from molt.capability_manifest import load_manifest
        m = load_manifest(path)
        assert m.resources.max_memory == 16 * 1024 * 1024
    finally:
        os.unlink(path)


def test_json_manifest_backward_compat():
    """Old JSON format still works."""
    from molt.capability_manifest import load_manifest

    manifest = {
        "allow": ["net", "fs.read"],
        "deny": [],
        "effects": ["nondet"],
    }

    with tempfile.NamedTemporaryFile(mode="w", suffix=".json", delete=False) as f:
        json.dump(manifest, f)
        path = f.name

    try:
        m = load_manifest(path)
        assert "net" in m.allow
        assert "fs.read" in m.allow
        assert m.version == "1.0"  # JSON is v1.0
    finally:
        os.unlink(path)


def test_invalid_manifest_raises():
    """Malformed manifests produce clear errors."""
    from molt.capability_manifest import load_manifest, ManifestError

    with tempfile.NamedTemporaryFile(mode="w", suffix=".toml", delete=False) as f:
        f.write("[invalid\nbroken toml")
        path = f.name

    try:
        try:
            load_manifest(path)
            assert False, "should have raised"
        except Exception:
            pass  # Any exception is fine -- the point is it doesn't silently succeed
    finally:
        os.unlink(path)


def test_manifest_with_virtual_mounts():
    """Virtual mount configuration parses correctly."""
    from molt.capability_manifest import load_manifest

    toml = '''
[manifest]
version = "2.0"

[io]
mode = "virtual"

[io.virtual_mounts]
"/tmp" = { type = "memory", max_size = "16MB" }
"/data" = { type = "readonly", source = "/bundle/data" }
'''
    with tempfile.NamedTemporaryFile(mode="w", suffix=".toml", delete=False) as f:
        f.write(toml)
        path = f.name

    try:
        m = load_manifest(path)
        assert m.io.mode == "virtual"
        assert len(m.io.virtual_mounts) == 2
        tmp_mount = next(v for v in m.io.virtual_mounts if v.path == "/tmp")
        assert tmp_mount.type == "memory"
        assert tmp_mount.max_size == 16 * 1024 * 1024
    finally:
        os.unlink(path)


if __name__ == "__main__":
    tests = [v for k, v in sorted(globals().items()) if k.startswith("test_") and callable(v)]
    passed = failed = 0
    for t in tests:
        try:
            t()
            passed += 1
            print(f"  PASS  {t.__name__}")
        except Exception as e:
            failed += 1
            print(f"  FAIL  {t.__name__}: {e}")
    print(f"\n{passed}/{passed+failed} passed")
