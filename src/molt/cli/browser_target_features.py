from __future__ import annotations

from collections.abc import Collection, Mapping
from typing import Any

from molt import target_features as _target_features
from molt.target_features import (
    TARGET_FEATURE_MANIFEST_ASSET_NAME,
    WEBGPU_DISPATCH_HOST_IMPORT,
)

__all__ = [
    "TARGET_FEATURE_MANIFEST_ASSET_NAME",
    "WEBGPU_DISPATCH_HOST_IMPORT",
    "browser_target_profile_for_imports",
    "browser_target_feature_metadata",
]


def browser_target_profile_for_imports(browser_host_import_names: Collection[str]) -> str:
    """Resolve the browser package target profile from required host imports."""

    if WEBGPU_DISPATCH_HOST_IMPORT in set(browser_host_import_names):
        return "wasm-browser-webgpu"
    return "wasm-browser"


def browser_target_feature_metadata(
    *,
    browser_host_import_names: Collection[str],
    manifest_asset: Mapping[str, Any],
) -> dict[str, Any]:
    """Build the browser package target-feature contract for manifest.json."""

    profile_id = browser_target_profile_for_imports(browser_host_import_names)
    profile = _target_features.target_profile(profile_id)
    summary = _target_features.target_profile_summary(profile_id)
    feature_catalog: dict[str, dict[str, Any]] = {
        feature["id"]: feature
        for feature in _target_features.target_feature_manifest()["features"]
    }
    feature_rows = []
    for row in profile["features"]:
        definition = feature_catalog[row["id"]]
        feature_rows.append(
            {
                "id": row["id"],
                "support": row["support"],
                "gate": row["gate"],
                "fallback": row["fallback"],
                "unsupported_reason": row["unsupported_reason"],
                "domain": definition["domain"],
                "tier": definition["tier"],
                "determinism": definition["determinism"],
                "description": definition["description"],
            }
        )

    return {
        "authority": "target_feature_manifest",
        "profile": profile["id"],
        "display_name": profile["display_name"],
        "family": profile["family"],
        "target_triple": profile["target_triple"],
        "artifact_kind": profile["artifact_kind"],
        "manifest_asset": dict(manifest_asset),
        "baseline_features": list(summary["baseline_features"]),
        "gated_features": list(summary["gated_features"]),
        "tracked_features": list(summary["tracked_features"]),
        "excluded_features": list(summary["excluded_features"]),
        "features": feature_rows,
        "required_host_imports": {
            "webgpu": sorted(
                name
                for name in browser_host_import_names
                if name == WEBGPU_DISPATCH_HOST_IMPORT
            )
        },
        "browser_probes": _browser_probe_plan(profile_id, feature_catalog),
    }


def _browser_probe_plan(
    profile_id: str,
    feature_catalog: Mapping[str, Mapping[str, Any]],
) -> dict[str, Any]:
    probes: dict[str, Any] = {
        "wasm": {
            "required": True,
            "module_validation": "WebAssembly.validate",
        }
    }
    _add_webgpu_probe_plan(probes, profile_id, feature_catalog)
    _add_webnn_probe_plan(probes, profile_id, feature_catalog)
    return probes


def _browser_probe(
    feature_catalog: Mapping[str, Mapping[str, Any]],
    feature_id: str,
) -> Mapping[str, Any]:
    raw = feature_catalog.get(feature_id, {}).get("browser_probe", {})
    return raw if isinstance(raw, Mapping) else {}


def _add_webgpu_probe_plan(
    probes: dict[str, Any],
    profile_id: str,
    feature_catalog: Mapping[str, Mapping[str, Any]],
) -> None:
    webgpu_support = _target_features.target_feature_support(profile_id, "webgpu.api")
    if webgpu_support == "excluded":
        probes["webgpu"] = {
            "required": False,
            "excluded_by_profile": True,
            "use_profile": "wasm-browser-webgpu",
        }
        return
    if webgpu_support is None:
        return

    webgpu_rows = [
        row
        for row in _target_features.target_profile(profile_id)["features"]
        if row["id"].startswith("webgpu.")
    ]
    api_probe = _browser_probe(feature_catalog, "webgpu.api")
    probes["webgpu"] = {
        "required": True,
        "api": api_probe["api"],
        "adapter": api_probe["adapter"],
        "device": api_probe["device"],
        "adapter_features": [
            {
                "feature": row["id"],
                "adapter_feature": probe["adapter_feature"],
                "support": row["support"],
                "fallback": row["fallback"],
                "unsupported_reason": row["unsupported_reason"],
            }
            for row in webgpu_rows
            if (probe := _browser_probe(feature_catalog, row["id"])).get("kind")
            == "adapter_feature"
        ],
        "shader_validation_features": [
            row["id"]
            for row in webgpu_rows
            if _browser_probe(feature_catalog, row["id"]).get("kind")
            in {"adapter_limits_and_shader_validation", "shader_validation"}
        ],
        "adapter_limits": [
            {
                "feature": row["id"],
                "limits": list(probe["adapter_limits"]),
            }
            for row in webgpu_rows
            if (probe := _browser_probe(feature_catalog, row["id"])).get(
                "adapter_limits"
            )
        ],
        "observability_features": [
            row["id"]
            for row in webgpu_rows
            if _browser_probe(feature_catalog, row["id"]).get("observability") is True
        ],
        "semantic_note": (
            "adapter limits and timestamp queries are observability inputs, "
            "not correctness facts"
        ),
    }


def _add_webnn_probe_plan(
    probes: dict[str, Any],
    profile_id: str,
    feature_catalog: Mapping[str, Mapping[str, Any]],
) -> None:
    webnn_support = _target_features.target_feature_support(profile_id, "webnn.graph")
    if webnn_support == "excluded":
        probes["webnn"] = {
            "required": False,
            "excluded_by_profile": True,
            "use_profile": "wasm-browser-webnn",
        }
        return
    if webnn_support is None:
        return

    webnn_rows = [
        row
        for row in _target_features.target_profile(profile_id)["features"]
        if row["id"].startswith("webnn.")
    ]
    webnn_probe = _browser_probe(feature_catalog, "webnn.graph")
    probes["webnn"] = {
        "required": True,
        "api": webnn_probe["api"],
        "context": webnn_probe["context"],
        "features": [
            {
                "feature": row["id"],
                "support": row["support"],
                "fallback": row["fallback"],
                "unsupported_reason": row["unsupported_reason"],
            }
            for row in webnn_rows
        ],
    }
