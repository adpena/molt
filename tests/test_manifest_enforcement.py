"""End-to-end test: verify manifest env vars propagate correctly."""
import sys
sys.path.insert(0, "src")


def test_manifest_env_var_propagation():
    """Verify that to_env_vars produces correct env vars."""
    from molt.capability_manifest import (
        CapabilityManifest, ResourceLimits, AuditConfig, IoConfig,
    )
    m = CapabilityManifest(
        allow=["net"],
        resources=ResourceLimits(max_memory=1048576, max_duration=5.0),
        audit=AuditConfig(enabled=True, sink="jsonl"),
        io=IoConfig(mode="virtual"),
    )
    env = m.to_env_vars()
    caps = set(env["MOLT_CAPABILITIES"].split(","))
    # "net" profile expands to net + websocket.connect + websocket.listen
    assert "net" in caps
    assert env["MOLT_RESOURCE_MAX_MEMORY"] == "1048576"
    assert env["MOLT_RESOURCE_MAX_DURATION_MS"] == "5000"
    assert env["MOLT_AUDIT_ENABLED"] == "1"
    assert env["MOLT_AUDIT_SINK"] == "jsonl"
    assert env["MOLT_IO_MODE"] == "virtual"


def test_manifest_env_vars_with_deny():
    """Verify deny removes capabilities from effective set."""
    from molt.capability_manifest import CapabilityManifest
    m = CapabilityManifest(allow=["net", "fs.read", "fs.write"], deny=["fs.write"])
    env = m.to_env_vars()
    caps = env["MOLT_CAPABILITIES"].split(",")
    assert "fs.write" not in caps
    assert "net" in caps
    assert "fs.read" in caps


def test_manifest_env_vars_omit_defaults():
    """Verify that default values don't produce env vars."""
    from molt.capability_manifest import CapabilityManifest
    m = CapabilityManifest()
    env = m.to_env_vars()
    assert "MOLT_RESOURCE_MAX_MEMORY" not in env
    assert "MOLT_AUDIT_ENABLED" not in env
    assert "MOLT_IO_MODE" not in env  # "real" is default, not set


def test_manifest_roundtrip_through_toml(tmp_path):
    """Write a manifest to TOML, load it, convert to env vars."""
    from molt.capability_manifest import load_manifest, CapabilityManifest
    import tomllib

    toml_content = '''
[manifest]
version = "2.0"

[capabilities]
allow = ["net", "env.read"]

[resources]
max_memory = "16MB"
max_duration = "10s"

[audit]
enabled = true
sink = "stderr"

[io]
mode = "virtual"
'''
    path = tmp_path / "test.toml"
    path.write_text(toml_content)

    m = load_manifest(str(path))
    env = m.to_env_vars()

    assert "net" in env["MOLT_CAPABILITIES"]
    assert env["MOLT_RESOURCE_MAX_MEMORY"] == str(16 * 1024 * 1024)
    assert env["MOLT_RESOURCE_MAX_DURATION_MS"] == "10000"
    assert env["MOLT_AUDIT_ENABLED"] == "1"
    assert env["MOLT_IO_MODE"] == "virtual"
