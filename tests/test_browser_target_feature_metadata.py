from __future__ import annotations

from molt.cli.browser_target_features import (
    TARGET_FEATURE_MANIFEST_ASSET_NAME,
    WEBGPU_DISPATCH_HOST_IMPORT,
    browser_target_feature_metadata,
    browser_target_profile_for_imports,
)


def _manifest_asset() -> dict[str, object]:
    return {
        "path": TARGET_FEATURE_MANIFEST_ASSET_NAME,
        "size": 123,
        "sha256": "0" * 64,
    }


def test_browser_target_profile_defaults_to_browser_wasm() -> None:
    assert browser_target_profile_for_imports(()) == "wasm-browser"
    data = browser_target_feature_metadata(
        browser_host_import_names=(),
        manifest_asset=_manifest_asset(),
    )

    assert data["authority"] == "target_feature_manifest"
    assert data["profile"] == "wasm-browser"
    assert data["manifest_asset"]["path"] == TARGET_FEATURE_MANIFEST_ASSET_NAME
    assert "wasm.mvp" in data["baseline_features"]
    assert "webgpu.api" in data["excluded_features"]
    assert data["required_host_imports"]["webgpu"] == []
    assert data["browser_probes"]["webgpu"] == {
        "required": False,
        "excluded_by_profile": True,
        "use_profile": "wasm-browser-webgpu",
    }
    assert data["browser_probes"]["webnn"] == {
        "required": False,
        "excluded_by_profile": True,
        "use_profile": "wasm-browser-webnn",
    }


def test_webgpu_host_import_selects_webgpu_profile_and_probe_contract() -> None:
    assert (
        browser_target_profile_for_imports((WEBGPU_DISPATCH_HOST_IMPORT,))
        == "wasm-browser-webgpu"
    )
    data = browser_target_feature_metadata(
        browser_host_import_names=("molt_log_host", WEBGPU_DISPATCH_HOST_IMPORT),
        manifest_asset=_manifest_asset(),
    )

    assert data["profile"] == "wasm-browser-webgpu"
    assert "webgpu.api" in data["gated_features"]
    assert "webnn.graph" in data["excluded_features"]
    assert data["required_host_imports"]["webgpu"] == [WEBGPU_DISPATCH_HOST_IMPORT]
    assert data["browser_probes"]["webnn"] == {
        "required": False,
        "excluded_by_profile": True,
        "use_profile": "wasm-browser-webnn",
    }

    webgpu_probe = data["browser_probes"]["webgpu"]
    assert webgpu_probe["required"] is True
    assert webgpu_probe["api"] == "navigator.gpu"
    assert webgpu_probe["adapter"] == "navigator.gpu.requestAdapter()"
    shader_f16 = next(
        row
        for row in webgpu_probe["adapter_features"]
        if row["feature"] == "webgpu.f16"
    )
    assert shader_f16["adapter_feature"] == "shader-f16"
    assert shader_f16["support"] == "gated"
    assert shader_f16["fallback"] == "f32_kernel"
    assert "requires adapter_feature_probe" in shader_f16["unsupported_reason"]
    assert "webgpu.wgsl_storage_buffers" in webgpu_probe["shader_validation_features"]
    assert "webgpu.timestamp_queries" in webgpu_probe["observability_features"]
    assert "not correctness facts" in webgpu_probe["semantic_note"]
