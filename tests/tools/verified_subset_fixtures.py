"""Synthetic receipt fixtures; real inventory custody is tested independently."""

from collections.abc import Callable
from dataclasses import replace
from pathlib import Path

from molt.verified_subset import (
    VerifiedSubsetCoordinate,
    capture_verified_subset_policy,
    verified_subset_coordinates,
)
from tools.compat.test_policy import CoordinateProjection, TestSourceInventory
from tools.verified_subset import VerifiedSubsetValidation
from tools import release_criterion_receipt, verified_subset


def synthetic_validation(
    repo_root: Path,
    project: Callable[[VerifiedSubsetCoordinate], CoordinateProjection],
) -> VerifiedSubsetValidation:
    policy_path = verified_subset.ROOT / "config" / "verified_subset.toml"
    policy, identity = capture_verified_subset_policy(policy_path)
    identity = replace(
        identity,
        sha256=release_criterion_receipt.stable_file_sha256(
            policy_path,
            label="synthetic policy input",
        ),
    )
    coordinates = verified_subset_coordinates(policy)
    inventory = TestSourceInventory(repo_root.resolve(), (), (), (), (), (), ())
    return VerifiedSubsetValidation(
        policy,
        identity,
        inventory,
        coordinates,
        tuple(project(cell) for cell in coordinates),
    )
