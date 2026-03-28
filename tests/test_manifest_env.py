import sys
sys.path.insert(0, "src")
from molt.capability_manifest import CapabilityManifest, ResourceLimits, AuditConfig, IoConfig

def test_manifest_to_env_vars_empty():
    m = CapabilityManifest()
    env = m.to_env_vars()
    assert env["MOLT_CAPABILITIES"] == ""
    assert "MOLT_RESOURCE_MAX_MEMORY" not in env

def test_manifest_to_env_vars_with_caps():
    m = CapabilityManifest(allow=["net", "fs.read"])
    env = m.to_env_vars()
    assert env["MOLT_CAPABILITIES"] == "fs.read,net,websocket.connect,websocket.listen"

def test_manifest_to_env_vars_with_resources():
    m = CapabilityManifest(resources=ResourceLimits(max_memory=67108864, max_duration=30.0, max_allocations=1000000, max_recursion_depth=500))
    env = m.to_env_vars()
    assert env["MOLT_RESOURCE_MAX_MEMORY"] == "67108864"
    assert env["MOLT_RESOURCE_MAX_DURATION_MS"] == "30000"
    assert env["MOLT_RESOURCE_MAX_ALLOCATIONS"] == "1000000"
    assert env["MOLT_RESOURCE_MAX_RECURSION_DEPTH"] == "500"

def test_manifest_to_env_vars_with_audit():
    m = CapabilityManifest(audit=AuditConfig(enabled=True, sink="jsonl", output="stderr"))
    env = m.to_env_vars()
    assert env["MOLT_AUDIT_ENABLED"] == "1"
    assert env["MOLT_AUDIT_SINK"] == "jsonl"

def test_manifest_to_env_vars_with_io_mode():
    m = CapabilityManifest(io=IoConfig(mode="virtual"))
    env = m.to_env_vars()
    assert env["MOLT_IO_MODE"] == "virtual"
