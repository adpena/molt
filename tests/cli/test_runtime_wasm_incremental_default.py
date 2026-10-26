"""Independent intent, target custody and execution constraint matrix."""

from __future__ import annotations

import pytest

from molt.cli.runtime_wasm_build_policy import (
    RuntimeWasmIncrementality as Incrementality,
    RuntimeWasmTargetLayout as Layout,
    resolve_runtime_wasm_build_policy,
)


@pytest.mark.parametrize(
    "intent,environment,layout,incrementality",
    [
        ("dev", {}, Layout.CHECKOUT, Incrementality.FORCED_OFF),
        ("release", {}, Layout.CHECKOUT, Incrementality.FORCED_OFF),
        (
            "release",
            {"MOLT_SESSION_ID": "proof"},
            Layout.SESSION,
            Incrementality.FORCED_OFF,
        ),
        (
            "release",
            {"CARGO_TARGET_DIR": "/caller"},
            Layout.EXPLICIT,
            Incrementality.FORCED_OFF,
        ),
        (
            "dev",
            {"CARGO_INCREMENTAL": "0"},
            Layout.CHECKOUT,
            Incrementality.FORCED_OFF,
        ),
        (
            "dev",
            {"MOLT_RUNTIME_WASM_INCREMENTAL": "0"},
            Layout.CHECKOUT,
            Incrementality.FORCED_OFF,
        ),
        (
            "release",
            {"MOLT_RUNTIME_WASM_INCREMENTAL": "1"},
            Layout.FAMILY,
            Incrementality.REQUESTED_ON,
        ),
        (
            "release",
            {"MOLT_RUNTIME_BUILD_PROFILE": "dev-fast"},
            Layout.FAMILY,
            Incrementality.REQUESTED_ON,
        ),
        (
            "release",
            {"MOLT_RUNTIME_BUILD_PROFILE": "not valid"},
            Layout.CHECKOUT,
            Incrementality.FORCED_OFF,
        ),
        (
            "dev",
            {"SOURCE_DATE_EPOCH": "123"},
            Layout.CHECKOUT,
            Incrementality.FORCED_OFF,
        ),
        (
            "dev",
            {
                "MOLT_PROOF_SCRATCH_ROOT": "/proof",
                "MOLT_SESSION_ID": "proof",
                "MOLT_RUNTIME_WASM_INCREMENTAL": "1",
            },
            Layout.SESSION,
            Incrementality.FORCED_OFF,
        ),
        (
            "release",
            {"CARGO_INCREMENTAL": "1"},
            Layout.CHECKOUT,
            Incrementality.PROFILE,
        ),
    ],
)
def test_runtime_wasm_policy_matrix(intent, environment, layout, incrementality):
    policy = resolve_runtime_wasm_build_policy(intent, environment)
    assert policy.build_profile == intent
    assert policy.layout is layout
    assert policy.incrementality is incrementality


def test_unresolved_cargo_profile_cannot_authorize_publication_or_diagnostics():
    policy = resolve_runtime_wasm_build_policy("dev", {})
    with pytest.raises(ValueError, match="resolved incrementality"):
        _ = policy.publish_shared_cache
    with pytest.raises(ValueError, match="resolved incrementality"):
        _ = policy.diagnostic_detail


@pytest.mark.parametrize("intent", ["dev", "release"])
def test_forced_off_request_still_requires_captured_cargo_policy(intent):
    policy = resolve_runtime_wasm_build_policy(intent, {"CARGO_INCREMENTAL": "0"})
    with pytest.raises(ValueError, match="resolved incrementality"):
        _ = policy.publish_shared_cache
    with pytest.raises(ValueError, match="resolved incrementality"):
        _ = policy.diagnostic_detail


@pytest.mark.parametrize(
    "environment,expected",
    [
        ({"CARGO_TARGET_DIR": "/caller"}, Layout.EXPLICIT),
        ({"MOLT_SESSION_ID": "owned-proof"}, Layout.SESSION),
        (
            {"MOLT_SESSION_ID": "generated", "MOLT_SESSION_ID_GENERATED": "1"},
            Layout.CHECKOUT,
        ),
        (
            {"MOLT_RUNTIME_WASM_INCREMENTAL": "1", "CARGO_TARGET_DIR": "/caller"},
            Layout.EXPLICIT,
        ),
    ],
)
def test_dev_iteration_preserves_canonical_target_custody(environment, expected):
    policy = resolve_runtime_wasm_build_policy("dev", environment)
    assert policy.layout is expected
