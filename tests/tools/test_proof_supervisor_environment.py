"""Native launch requirements stay singular and enter every environment receipt."""

from __future__ import annotations

import json
from pathlib import Path
import subprocess
import sys

import pytest

from tools.proof_queue_pkg import execution_environment, supervisor_custody


def _capability(required: object) -> dict[str, object]:
    return {
        "schema": "molt.proof-supervisor-capability.v2",
        "platform": {"win32": "windows", "darwin": "macos"}.get(
            sys.platform, sys.platform
        ),
        "mode": "declared-tree",
        "backend": "test-native-backend",
        "available": True,
        "pre_entry_exec_authority": True,
        "recursive_descendant_authority": True,
        "reason": None,
        "required_environment": required,
    }


def _read_capability(
    monkeypatch: pytest.MonkeyPatch, payload: object
) -> dict[str, str]:
    def capture(
        command: tuple[str, ...], **kwargs: object
    ) -> subprocess.CompletedProcess[str]:
        assert command == ("supervisor.bin", "capability", "declared-tree")
        assert kwargs["timeout"] == 30.0
        return subprocess.CompletedProcess(command, 0, json.dumps(payload), "")

    monkeypatch.setattr(supervisor_custody.command_identity, "_run_captured", capture)
    return supervisor_custody.required_execution_environment(
        binary=Path("supervisor.bin"), mode="declared-tree", cwd=Path.cwd(), env={}
    )


@pytest.mark.parametrize("required", [{}, {"_NO_DEBUG_HEAP": "1"}])
def test_native_capability_owns_platform_launch_environment(
    monkeypatch: pytest.MonkeyPatch, required: dict[str, str]
) -> None:
    assert _read_capability(monkeypatch, _capability(required)) == required


@pytest.mark.parametrize(
    ("field", "value"),
    [
        ("schema", "molt.proof-supervisor-capability.v1"),
        ("platform", "foreign-platform"),
        ("mode", "leaf"),
        ("backend", ""),
        ("available", 1),
        ("available", False),
        ("pre_entry_exec_authority", False),
        ("recursive_descendant_authority", False),
        ("reason", []),
        ("unknown", "field"),
        ("required_environment", None),
        ("required_environment", []),
        ("required_environment", {"_NO_DEBUG_HEAP": True}),
        ("required_environment", {"_NO_DEBUG_HEAP": ""}),
        ("required_environment", {"_NO_DEBUG_HEAP": "1\n"}),
        ("required_environment", {"_NO_DEBUG_HEAP": "1\0"}),
        ("required_environment", {"_no_debug_heap": "1"}),
        ("required_environment", {"NOT=AN_ENV_NAME": "1"}),
    ],
)
def test_native_launch_capability_rejects_untyped_or_foreign_data(
    monkeypatch: pytest.MonkeyPatch, field: str, value: object
) -> None:
    payload = _capability({})
    payload[field] = value
    with pytest.raises(ValueError, match="native supervisor"):
        _read_capability(monkeypatch, payload)


def test_required_launch_environment_is_captured_and_cannot_be_overridden() -> None:
    required = {"_NO_DEBUG_HEAP": "1"}
    selected, contract = execution_environment._deterministic_execution_environment(
        {"PATH": "host-tools", "UNCLASSIFIED": "discard"},
        override_names=[],
        required_environment=required,
    )
    assert selected == {"PATH": "host-tools", **required}
    assert contract["passed_names"] == ["_NO_DEBUG_HEAP", "PATH"]
    assert contract["supervisor_owned_names"] == ["_NO_DEBUG_HEAP"]
    assert contract["omitted_names"] == ["UNCLASSIFIED"]
    authority = execution_environment._execution_environment_authority(
        selected,
        applied_cargo_policies=(),
        fingerprint_key=b"x" * 32,
        contract=contract,
    )
    assert authority["canonical_values_sha256"] == (
        execution_environment._canonical_environment_sha256(selected)
    )
    assert (
        authority["variables"]["_NO_DEBUG_HEAP"]["class"] == "supervisor-owned-launch"
    )
    assert authority["variables"]["_NO_DEBUG_HEAP"]["redacted"] is True
    for name in ("_NO_DEBUG_HEAP", "_no_debug_heap"):
        with pytest.raises(ValueError, match="cannot be overridden"):
            execution_environment._deterministic_execution_environment(
                {name: "1"}, override_names=[name], required_environment=required
            )


def test_inventory_and_execution_share_canonical_binding() -> None:
    required = {"_NO_DEBUG_HEAP": "1"}
    assert supervisor_custody.bind_required_environment(
        {"_no_debug_heap": "1", "PATH": "tools"}, required
    ) == {"PATH": "tools", **required}
    with pytest.raises(ValueError, match="conflicts"):
        supervisor_custody.bind_required_environment({"_no_debug_heap": "0"}, required)
    with pytest.raises(ValueError, match="case-ambiguous"):
        supervisor_custody.bind_required_environment(
            {"_NO_DEBUG_HEAP": "1", "_no_debug_heap": "1"}, required
        )


def test_no_platform_requirement_does_not_admit_windows_environment() -> None:
    selected, contract = execution_environment._deterministic_execution_environment(
        {"_NO_DEBUG_HEAP": "1", "PATH": "tools"},
        override_names=[],
        required_environment={},
    )
    assert selected == {"PATH": "tools"}
    assert contract["omitted_names"] == ["_NO_DEBUG_HEAP"]
    assert "supervisor_owned_names" not in contract


def test_native_requirement_is_cross_bound_to_receipt_policy_and_ownership() -> None:
    required = {"_NO_DEBUG_HEAP": "1"}
    supervisor_custody.validate_required_environment_binding(
        required_environment=required,
        supervisor_metadata=required,
        policy_environment={"PATH": "tools", **required},
        supervisor_owned_names=["_NO_DEBUG_HEAP"],
    )
    substitutions = (
        (
            None,
            {"PATH": "tools", **required},
            ["_NO_DEBUG_HEAP"],
            "required-environment metadata mismatch",
        ),
        (
            required,
            {"PATH": "tools", "_NO_DEBUG_HEAP": "0"},
            ["_NO_DEBUG_HEAP"],
            "required environment differs from policy",
        ),
        (
            required,
            {"PATH": "tools", **required},
            [],
            "environment ownership metadata mismatch",
        ),
    )
    for metadata, policy, owned_names, error in substitutions:
        with pytest.raises(ValueError, match=error):
            supervisor_custody.validate_required_environment_binding(
                required_environment=required,
                supervisor_metadata=metadata,
                policy_environment=policy,
                supervisor_owned_names=owned_names,
            )
