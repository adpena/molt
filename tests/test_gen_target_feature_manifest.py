"""Freshness and drift guards for the target feature manifest fact."""

from __future__ import annotations

import copy
import importlib.util
import json
import sys
from pathlib import Path

import pytest

from molt import target_features as TF


ROOT = Path(__file__).resolve().parents[1]
GEN = ROOT / "tools" / "gen_target_feature_manifest.py"
OUT_PY = ROOT / "src" / "molt" / "_target_feature_manifest.py"
OUT_JSON = ROOT / "wasm" / "target_feature_manifest.json"


def _load_generator():
    spec = importlib.util.spec_from_file_location(
        "molt_test_gen_target_feature_manifest", GEN
    )
    assert spec is not None and spec.loader is not None
    module = importlib.util.module_from_spec(spec)
    sys.modules["molt_test_gen_target_feature_manifest"] = module
    spec.loader.exec_module(module)
    return module


def test_target_feature_manifest_is_in_sync() -> None:
    gen = _load_generator()
    model = gen.build_model()
    assert OUT_PY.read_text(encoding="utf-8") == gen.render_python(model)
    assert OUT_JSON.read_text(encoding="utf-8") == gen.render_json(model)


def test_check_mode_is_green() -> None:
    gen = _load_generator()
    assert gen.main(["--check"]) == 0


def test_public_api_exposes_doc_71_target_profiles() -> None:
    assert TF.target_profile_ids() == (
        "native",
        "wasm-server",
        "wasm-browser",
        "wasm-edge",
        "wasm-browser-webgpu",
        "wasm-browser-webnn",
    )
    assert TF.target_baseline_features("wasm-browser") == (
        "wasm.mvp",
        "tensor.typed_strided_storage",
    )
    assert TF.target_supports("wasm-browser", "wasm.simd128") is True
    assert TF.target_feature_support("wasm-browser", "wasm.simd128") == "gated"
    assert TF.target_feature_support("native", "numeric.blas") == "gated"
    assert TF.target_feature_support("native", "numeric.lapack") == "gated"
    assert TF.target_feature_support("native", "numeric.gsl") == "gated"
    assert TF.target_feature_support("wasm-browser", "numeric.blas") == "tracked"


def test_target_rows_carry_doc_71_feature_envelope_and_diagnostics() -> None:
    manifest = TF.target_feature_manifest()
    feature_defs = {feature["id"]: feature for feature in manifest["features"]}
    by_id = {target["id"]: target for target in manifest["targets"]}
    wasm_browser = by_id["wasm-browser"]
    feature_rows = {row["id"]: row for row in wasm_browser["features"]}

    for feature_id in (
        "wasm.tail_call",
        "wasm.exception_handling",
        "wasm.memory64",
        "wasm.threads",
        "wasm.atomics",
        "wasm.gc",
        "wasm.reference_types",
        "wasi.component_model",
        "wasi.async_0_3",
        "numeric.blas",
        "numeric.lapack",
        "numeric.gsl",
    ):
        assert feature_id in feature_rows
        assert feature_rows[feature_id]["unsupported_reason"]

    assert feature_rows["webgpu.f16"]["support"] == "excluded"
    assert "use fallback use_wasm-browser-webgpu" in (
        feature_rows["webgpu.f16"]["unsupported_reason"]
    )
    assert feature_defs["webgpu.api"]["browser_probe"] == {
        "kind": "webgpu_api",
        "api": "navigator.gpu",
        "adapter": "navigator.gpu.requestAdapter()",
        "device": "adapter.requestDevice()",
    }
    assert feature_defs["webgpu.f16"]["browser_probe"]["adapter_feature"] == (
        "shader-f16"
    )
    assert feature_defs["webgpu.workgroup_memory"]["browser_probe"][
        "adapter_limits"
    ] == [
        "maxComputeInvocationsPerWorkgroup",
        "maxComputeWorkgroupSizeX",
        "maxComputeWorkgroupSizeY",
        "maxComputeWorkgroupSizeZ",
        "maxComputeWorkgroupStorageSize",
    ]
    assert feature_defs["webgpu.timestamp_queries"]["browser_probe"][
        "observability"
    ] is True


def test_webgpu_and_webnn_profiles_are_separate() -> None:
    assert TF.target_feature_support("wasm-browser", "webgpu.api") == "excluded"
    assert TF.target_feature_support("wasm-browser", "webnn.graph") == "excluded"
    assert TF.target_supports("wasm-browser", "webgpu.api") is False
    assert TF.target_supports("wasm-browser", "webnn.graph") is False

    assert TF.target_feature_support("wasm-browser-webgpu", "webgpu.api") == "gated"
    assert TF.target_feature_support("wasm-browser-webgpu", "webnn.graph") == "excluded"
    assert TF.target_supports("wasm-browser-webgpu", "webgpu.api") is True
    assert TF.target_supports("wasm-browser-webgpu", "webnn.graph") is False

    assert TF.target_feature_support("wasm-browser-webnn", "webnn.graph") == "gated"
    assert TF.target_feature_support("wasm-browser-webnn", "webgpu.api") == "excluded"
    assert TF.target_supports("wasm-browser-webnn", "webnn.graph") is True
    assert TF.target_supports("wasm-browser-webnn", "webgpu.api") is False


def test_json_artifact_matches_public_manifest() -> None:
    checked_in = json.loads(OUT_JSON.read_text(encoding="utf-8"))
    assert checked_in == TF.target_feature_manifest()


def test_generator_rejects_unknown_target_feature() -> None:
    gen = _load_generator()
    data = copy.deepcopy(gen.load_source())
    data["target"][0]["feature"][0]["id"] = "wasm.ghost_feature"
    with pytest.raises(gen.TargetFeatureManifestError, match="unknown feature"):
        gen.build_model(data)


def test_generator_rejects_relaxed_simd_as_baseline() -> None:
    gen = _load_generator()
    data = copy.deepcopy(gen.load_source())
    for target in data["target"]:
        if target["id"] == "wasm-browser":
            for feature in target["feature"]:
                if feature["id"] == "wasm.relaxed_simd":
                    feature["support"] = "baseline"
                    break
            break
    with pytest.raises(gen.TargetFeatureManifestError, match="relaxed_simd"):
        gen.build_model(data)


def test_generator_rejects_webgpu_webnn_conflation() -> None:
    gen = _load_generator()
    data = copy.deepcopy(gen.load_source())
    for target in data["target"]:
        if target["id"] == "wasm-browser-webgpu":
            for feature in target["feature"]:
                if feature["id"] == "webnn.graph":
                    feature["support"] = "gated"
                    break
            break
    with pytest.raises(gen.TargetFeatureManifestError, match="exclude WebNN"):
        gen.build_model(data)


def test_generator_rejects_missing_doc_71_wasm_feature_envelope() -> None:
    gen = _load_generator()
    data = copy.deepcopy(gen.load_source())
    for target in data["target"]:
        if target["id"] == "wasm-browser":
            target["feature"] = [
                row
                for row in target["feature"]
                if row["id"] != "wasm.exception_handling"
            ]
            break
    with pytest.raises(gen.TargetFeatureManifestError, match="WASM profile envelope"):
        gen.build_model(data)


def test_generator_rejects_missing_webgpu_browser_probe() -> None:
    gen = _load_generator()
    data = copy.deepcopy(gen.load_source())
    for feature in data["feature"]:
        if feature["id"] == "webgpu.f16":
            feature.pop("browser_probe")
            break
    with pytest.raises(gen.TargetFeatureManifestError, match="browser probe"):
        gen.build_model(data)
