import sys

import pytest

sys.path.insert(0, "src")
from molt.capability_manifest import (
    CapabilityManifest,
    ManifestError,
    ResourceLimits,
    AuditConfig,
    IoConfig,
)


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
    m = CapabilityManifest(
        resources=ResourceLimits(
            max_memory=67108864,
            max_duration=30.0,
            max_allocations=1000000,
            max_recursion_depth=500,
        )
    )
    env = m.to_env_vars()
    assert env["MOLT_RESOURCE_MAX_MEMORY"] == "67108864"
    assert env["MOLT_RESOURCE_MAX_DURATION_MS"] == "30000"
    assert env["MOLT_RESOURCE_MAX_ALLOCATIONS"] == "1000000"
    assert env["MOLT_RESOURCE_MAX_RECURSION_DEPTH"] == "500"


def test_manifest_to_env_vars_rejects_invalid_resource_limits():
    m = CapabilityManifest(resources=ResourceLimits(max_memory=0))

    with pytest.raises(ManifestError, match="max_memory must be positive"):
        m.to_env_vars()


def test_manifest_to_env_vars_emits_per_operation_caps():
    """Per-op caps must reach the env boundary; they must NOT be silently
    dropped (the Python<->Rust ResourceLimits asymmetry this closes)."""
    m = CapabilityManifest(
        resources=ResourceLimits(
            max_pow_result=1048576,
            max_repeat_result=2097152,
            max_shift_result=3145728,
            max_string_result=4194304,
        )
    )
    env = m.to_env_vars()
    assert env["MOLT_RESOURCE_MAX_POW_RESULT"] == "1048576"
    assert env["MOLT_RESOURCE_MAX_REPEAT_RESULT"] == "2097152"
    assert env["MOLT_RESOURCE_MAX_SHIFT_RESULT"] == "3145728"
    assert env["MOLT_RESOURCE_MAX_STRING_RESULT"] == "4194304"


def test_manifest_per_operation_caps_no_silent_field_drop():
    """Every Optional ResourceLimits field that is set MUST produce a
    MOLT_RESOURCE_MAX_* env var (the field-drop regression guard). The memory
    field is exercised via its canonical raw-byte name."""
    rl = ResourceLimits(
        max_memory=67108864,
        max_duration=30.0,
        max_allocations=1000000,
        max_recursion_depth=500,
        max_pow_result=1048576,
        max_repeat_result=2097152,
        max_shift_result=3145728,
        max_string_result=4194304,
    )
    env = CapabilityManifest(resources=rl).to_env_vars()

    # Map each populated dataclass field to the env var it must reach.
    field_to_env = {
        "max_memory": "MOLT_RESOURCE_MAX_MEMORY",
        "max_duration": "MOLT_RESOURCE_MAX_DURATION_MS",
        "max_allocations": "MOLT_RESOURCE_MAX_ALLOCATIONS",
        "max_recursion_depth": "MOLT_RESOURCE_MAX_RECURSION_DEPTH",
        "max_pow_result": "MOLT_RESOURCE_MAX_POW_RESULT",
        "max_repeat_result": "MOLT_RESOURCE_MAX_REPEAT_RESULT",
        "max_shift_result": "MOLT_RESOURCE_MAX_SHIFT_RESULT",
        "max_string_result": "MOLT_RESOURCE_MAX_STRING_RESULT",
    }
    # Ensure no per-op field is missing from the mapping (catches a future
    # field added to the dataclass without an env serialization).
    import dataclasses

    for f in dataclasses.fields(rl):
        assert f.name in field_to_env, (
            f"ResourceLimits field {f.name!r} has no env mapping — it would be "
            f"silently dropped at the env boundary"
        )
    for field_name, env_name in field_to_env.items():
        if getattr(rl, field_name) is not None:
            assert env_name in env, f"{field_name} dropped at env boundary"


def test_manifest_rejects_non_positive_per_operation_caps():
    m = CapabilityManifest(resources=ResourceLimits(max_pow_result=0))
    with pytest.raises(ManifestError, match="max_pow_result must be positive"):
        m.to_env_vars()


def test_manifest_to_env_vars_with_audit():
    m = CapabilityManifest(
        audit=AuditConfig(enabled=True, sink="jsonl", output="stderr")
    )
    env = m.to_env_vars()
    assert env["MOLT_AUDIT_ENABLED"] == "1"
    assert env["MOLT_AUDIT_SINK"] == "jsonl"


def test_manifest_to_env_vars_with_io_mode():
    m = CapabilityManifest(io=IoConfig(mode="virtual"))
    env = m.to_env_vars()
    assert env["MOLT_IO_MODE"] == "virtual"
