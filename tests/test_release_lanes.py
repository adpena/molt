"""Release-lane source and projection oracles; no target execution claims."""

from dataclasses import FrozenInstanceError
from pathlib import Path

import pytest

from molt.release_lanes import capture_release_lanes
from tests.release_lane_fixtures import (
    EXPECTED_LANES,
    LANE_FIELDS,
    stage_release_lane_authorities,
)

ROOT = Path(__file__).resolve().parents[1]


def test_required_lanes_are_exact_and_native_llvm_share_only_the_physical_target():
    inventory = capture_release_lanes(ROOT)
    assert (
        tuple(
            tuple(lane.as_record()[key] for key in LANE_FIELDS)
            for lane in inventory.lanes
        )
        == EXPECTED_LANES
    )
    assert len({lane.id for lane in inventory.lanes}) == 11
    native = inventory.select(backend="native", runtime_profile="release-size")
    llvm = inventory.select(backend="llvm", runtime_profile="release-size")
    assert native.target == llvm.target == "native"
    assert native.runtime_profile == llvm.runtime_profile == "release-size"
    assert native.id != llvm.id
    with pytest.raises(FrozenInstanceError):
        llvm.backend = "native"


@pytest.mark.parametrize(
    "backend,profile",
    [
        ("wasm", "release-fast"),
        ("native", "wasm-release"),
        ("llvm", "dev"),
        ("rust", "release-output"),
    ],
)
def test_unsupported_lane_never_becomes_a_nearby_profile(backend, profile):
    with pytest.raises(ValueError, match="unsupported release lane"):
        capture_release_lanes(ROOT).select(backend=backend, runtime_profile=profile)


@pytest.mark.parametrize(
    "mutation,diagnostic",
    [
        (
            lambda text: text.replace(
                'compiler_profile = "release"',
                'compiler_profile = "no-such-cargo-profile"',
                1,
            ),
            "unknown compiler_profile",
        ),
        (
            lambda text: text.replace(
                'backend = "native", guest_profile = "dev"',
                'backend = "rust", guest_profile = "dev"',
                1,
            ),
            "outside declared runtime support",
        ),
        (
            lambda text: text.replace(
                'backend = "native", guest_profile = "dev"',
                'backend = "native", target = "wasm", guest_profile = "dev"',
                1,
            ),
            "lane is not exact",
        ),
        (
            lambda text: text.replace(
                'runtime_profile = "release-size"',
                'runtime_profile = "release-output"',
                1,
            ),
            "duplicate release matrix lane",
        ),
        (
            lambda text: text.replace(
                'authority = "src/molt/cli/cargo_profiles.py"',
                'authority = "../outside.py"',
                1,
            ),
            "portable relative",
        ),
    ],
)
def test_malformed_config_is_rejected_by_the_shared_reader(
    tmp_path, mutation, diagnostic
):
    stage_release_lane_authorities(tmp_path)
    path = tmp_path / "config/release_acceptance_matrix.toml"
    path.write_text(mutation(path.read_text("utf-8")), "utf-8")
    with pytest.raises(ValueError, match=diagnostic):
        capture_release_lanes(tmp_path)


def test_captured_lane_authorities_keep_the_existing_mutation_fence(tmp_path):
    stage_release_lane_authorities(tmp_path)
    inventory = capture_release_lanes(tmp_path)
    path = tmp_path / "src/molt/cli/cargo_profiles.py"
    path.write_bytes(path.read_bytes() + b"\n# changed source authority\n")
    with pytest.raises((OSError, ValueError)):
        inventory.verify()


def test_lane_environment_selects_runtime_and_compiler_independently():
    lane = capture_release_lanes(ROOT).select(
        backend="llvm", runtime_profile="release-size"
    )
    assert lane.build_args() == ("--backend", "llvm")
    assert lane.environment() == {
        "MOLT_BACKEND_PROFILE": "release",
        "MOLT_RELEASE_BACKEND_CARGO_PROFILE": "release",
        "MOLT_DEV_BACKEND_CARGO_PROFILE": "release",
        "MOLT_RELEASE_CARGO_PROFILE": "release-size",
        "MOLT_DEV_CARGO_PROFILE": "release-size",
        "MOLT_WASM_CARGO_PROFILE": "release-size",
        "MOLT_RUNTIME_BUILD_PROFILE": "",
        "MOLT_RUNTIME_WASM_INCREMENTAL": "0",
    }


@pytest.mark.parametrize(
    "backend,profile,selected",
    [
        ("native", "release-fast", "cranelift"),
        ("llvm", "release-fast", "llvm"),
        ("wasm", "wasm-release", "cranelift"),
    ],
)
def test_lane_selects_codegen_by_flag_and_replaces_ambient_profiles(
    backend, profile, selected
):
    lane = capture_release_lanes(ROOT).select(backend=backend, runtime_profile=profile)
    env = {"MOLT_RUNTIME_BUILD_PROFILE": "unrelated", **lane.environment()}
    assert lane.build_args() == ("--backend", selected)
    assert "MOLT_BACKEND" not in lane.environment()
    assert env["MOLT_RUNTIME_BUILD_PROFILE"] == ""
    assert lane.compiler_features == (
        ("native-backend", "llvm")
        if backend == "llvm"
        else ("wasm-backend",)
        if backend == "wasm"
        else ("native-backend",)
    )
