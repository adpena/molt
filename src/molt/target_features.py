"""Public accessors for Molt target/profile feature facts."""

from __future__ import annotations

from typing import Any

from molt._target_feature_manifest import (
    BROWSER_TARGET_FAMILY,
    TARGET_FEATURE_MANIFEST_ASSET_NAME,
    WEBGPU_DISPATCH_HOST_IMPORT,
    target_feature_constants,
    target_feature_manifest,
    target_feature_row,
    target_feature_support,
    target_profile,
    target_profile_ids,
)

__all__ = (
    "BROWSER_TARGET_FAMILY",
    "TARGET_FEATURE_MANIFEST_ASSET_NAME",
    "WEBGPU_DISPATCH_HOST_IMPORT",
    "target_baseline_features",
    "target_feature_constants",
    "target_feature_manifest",
    "target_feature_row",
    "target_feature_support",
    "target_gated_features",
    "target_profile",
    "target_profile_ids",
    "target_profile_summary",
    "target_supports",
)


def target_supports(profile_id: str, feature_id: str) -> bool:
    """Return true when a profile may use a feature after its declared gate passes."""

    support = target_feature_support(profile_id, feature_id)
    return support in {"baseline", "gated"}


def target_baseline_features(profile_id: str) -> tuple[str, ...]:
    """Feature ids available without an extra target/runtime probe."""

    profile = target_profile(profile_id)
    return tuple(
        row["id"] for row in profile["features"] if row["support"] == "baseline"
    )


def target_gated_features(profile_id: str) -> tuple[str, ...]:
    """Feature ids available only after the profile row's gate proves support."""

    profile = target_profile(profile_id)
    return tuple(row["id"] for row in profile["features"] if row["support"] == "gated")


def target_profile_summary(profile_id: str) -> dict[str, Any]:
    """Small summary used by diagnostics and scoreboards."""

    profile = target_profile(profile_id)
    features = profile["features"]
    return {
        "id": profile["id"],
        "family": profile["family"],
        "target_triple": profile["target_triple"],
        "artifact_kind": profile["artifact_kind"],
        "baseline_features": tuple(
            row["id"] for row in features if row["support"] == "baseline"
        ),
        "gated_features": tuple(
            row["id"] for row in features if row["support"] == "gated"
        ),
        "tracked_features": tuple(
            row["id"] for row in features if row["support"] == "tracked"
        ),
        "excluded_features": tuple(
            row["id"] for row in features if row["support"] == "excluded"
        ),
    }
