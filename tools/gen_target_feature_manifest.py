#!/usr/bin/env python3
"""Generate the target feature manifest facts from the checked-in TOML source.

The source table records the target/profile feature ceilings introduced by
docs/design/foundation/71_wasm_webgpu_numeric_acceleration.md. The generated
Python and JSON artifacts are the consumer surface for packaging, scoreboards,
and future lowering gates; the TOML remains the reviewable authority.

Usage:

    python3 tools/gen_target_feature_manifest.py
    python3 tools/gen_target_feature_manifest.py --check
"""

from __future__ import annotations

import argparse
import copy
import json
import re
import sys
import tomllib
from pathlib import Path
from typing import Any

ROOT = Path(__file__).resolve().parents[1]
SOURCE_TOML = ROOT / "src/molt/target_feature_manifest.toml"
OUT_PY = ROOT / "src/molt/_target_feature_manifest.py"
OUT_JSON = ROOT / "wasm/target_feature_manifest.json"
OUT_JS = ROOT / "wasm/target_feature_constants.generated.js"

EXPECTED_TARGET_IDS: tuple[str, ...] = (
    "native",
    "wasm-server",
    "wasm-browser",
    "wasm-edge",
    "wasm-browser-webgpu",
    "wasm-browser-webnn",
)

NUMERIC_LIBRARY_FEATURE_IDS: tuple[str, ...] = (
    "numeric.blas",
    "numeric.lapack",
    "numeric.gsl",
)

WASM_PROFILE_FEATURE_IDS: tuple[str, ...] = (
    "wasm.mvp",
    "wasm.multi_value",
    "wasm.simd128",
    "wasm.relaxed_simd",
    "wasm.flexible_vectors",
    "wasm.half_precision",
    "wasm.wide_arithmetic",
    "wasm.tail_call",
    "wasm.exception_handling",
    "wasm.memory64",
    "wasm.threads",
    "wasm.atomics",
    "wasm.gc",
    "wasm.reference_types",
    "wasm.js_promise_integration",
    "wasi.preview1",
    "wasi.component_model",
    "wasi.async_0_3",
    *NUMERIC_LIBRARY_FEATURE_IDS,
    "tensor.typed_strided_storage",
)

WEBGPU_FEATURE_IDS: tuple[str, ...] = (
    "webgpu.api",
    "webgpu.wgsl_storage_buffers",
    "webgpu.workgroup_memory",
    "webgpu.atomics",
    "webgpu.f16",
    "webgpu.subgroups",
    "webgpu.timestamp_queries",
)

# Derived-fact keys emitted as the single-source browser/target-feature constants
# in both generated artifacts. Consumers must read these from the generated
# facts rather than redeclaring the literals locally.
BROWSER_TARGET_FAMILY = "wasm-browser"
WEBGPU_HOST_IMPORT_KIND = "webgpu"
WEBGPU_DISPATCH_PROFILE_ID = "wasm-browser-webgpu"
TARGET_FEATURE_MANIFEST_ASSET_NAME = OUT_JSON.name

VALID_SUPPORT = frozenset({"baseline", "gated", "tracked", "excluded"})
VALID_DETERMINISM = frozenset(
    {
        "deterministic",
        "non_bit_exact",
        "observability",
        "device_dependent",
        "external_numeric",
        "external_provider",
        "host_capability",
    }
)
VALID_BROWSER_PROBE_KIND = frozenset(
    {
        "adapter_feature",
        "adapter_limits_and_shader_validation",
        "shader_validation",
        "webgpu_api",
        "webnn_api",
    }
)

_ID_RE = re.compile(r"^[a-z0-9][a-z0-9_.-]*$")


class TargetFeatureManifestError(RuntimeError):
    """Raised when the target feature manifest source is structurally invalid."""


def _require_str(row: dict[str, Any], field: str, *, context: str) -> str:
    value = row.get(field)
    if not isinstance(value, str) or not value:
        raise TargetFeatureManifestError(
            f"{context}: {field} must be a non-empty string"
        )
    return value


def _optional_str(row: dict[str, Any], field: str, *, context: str) -> str | None:
    value = row.get(field)
    if value is None:
        return None
    if not isinstance(value, str) or not value:
        raise TargetFeatureManifestError(
            f"{context}: {field} must be a non-empty string when set"
        )
    return value


def _optional_str_list(
    row: dict[str, Any],
    field: str,
    *,
    context: str,
) -> list[str] | None:
    value = row.get(field)
    if value is None:
        return None
    if not isinstance(value, list) or not value:
        raise TargetFeatureManifestError(
            f"{context}: {field} must be a non-empty string list when set"
        )
    out: list[str] = []
    for item in value:
        if not isinstance(item, str) or not item:
            raise TargetFeatureManifestError(
                f"{context}: {field} entries must be non-empty strings"
            )
        out.append(item)
    return out


def _optional_bool(row: dict[str, Any], field: str, *, context: str) -> bool | None:
    value = row.get(field)
    if value is None:
        return None
    if not isinstance(value, bool):
        raise TargetFeatureManifestError(f"{context}: {field} must be boolean")
    return value


def _optional_str_list_table(
    row: dict[str, Any],
    field: str,
    *,
    context: str,
) -> dict[str, list[str]] | None:
    value = row.get(field)
    if value is None:
        return None
    if not isinstance(value, dict):
        raise TargetFeatureManifestError(f"{context}: {field} must be a table")
    out: dict[str, list[str]] = {}
    for key, raw_items in value.items():
        if not isinstance(key, str) or not key:
            raise TargetFeatureManifestError(
                f"{context}: {field} keys must be non-empty strings"
            )
        if not isinstance(raw_items, list):
            raise TargetFeatureManifestError(
                f"{context}: {field}.{key} must be a string list"
            )
        items: list[str] = []
        seen: set[str] = set()
        for item in raw_items:
            if not isinstance(item, str) or not item:
                raise TargetFeatureManifestError(
                    f"{context}: {field}.{key} entries must be non-empty strings"
                )
            if item in seen:
                raise TargetFeatureManifestError(
                    f"{context}: {field}.{key} duplicate import {item!r}"
                )
            seen.add(item)
            items.append(item)
        out[key] = items
    return out


def _check_id(value: str, *, context: str) -> None:
    if _ID_RE.fullmatch(value) is None:
        raise TargetFeatureManifestError(
            f"{context}: id {value!r} must match {_ID_RE.pattern}"
        )


def _browser_probe_row(feature_id: str, raw: dict[str, Any]) -> dict[str, Any] | None:
    probe_raw = raw.get("browser_probe")
    if probe_raw is None:
        return None
    if not isinstance(probe_raw, dict):
        raise TargetFeatureManifestError(
            f"feature {feature_id}: browser_probe must be a table"
        )
    kind = _require_str(
        probe_raw,
        "kind",
        context=f"feature {feature_id} browser_probe",
    )
    if kind not in VALID_BROWSER_PROBE_KIND:
        raise TargetFeatureManifestError(
            f"feature {feature_id}: browser_probe kind {kind!r} must be one of "
            f"{sorted(VALID_BROWSER_PROBE_KIND)}"
        )
    out: dict[str, Any] = {"kind": kind}
    for field in ("api", "adapter", "device", "context", "adapter_feature"):
        value = _optional_str(
            probe_raw,
            field,
            context=f"feature {feature_id} browser_probe",
        )
        if value is not None:
            out[field] = value
    adapter_limits = _optional_str_list(
        probe_raw,
        "adapter_limits",
        context=f"feature {feature_id} browser_probe",
    )
    if adapter_limits is not None:
        out["adapter_limits"] = adapter_limits
    observability = probe_raw.get("observability")
    if observability is not None:
        if not isinstance(observability, bool):
            raise TargetFeatureManifestError(
                f"feature {feature_id} browser_probe: observability must be boolean"
            )
        out["observability"] = observability

    if kind == "adapter_feature" and "adapter_feature" not in out:
        raise TargetFeatureManifestError(
            f"feature {feature_id}: adapter_feature browser probe must name adapter_feature"
        )
    if kind == "adapter_limits_and_shader_validation" and "adapter_limits" not in out:
        raise TargetFeatureManifestError(
            f"feature {feature_id}: adapter-limits browser probe must name adapter_limits"
        )
    if kind == "webgpu_api":
        for field in ("api", "adapter", "device"):
            if field not in out:
                raise TargetFeatureManifestError(
                    f"feature {feature_id}: webgpu_api browser probe must name {field}"
                )
    if kind == "webnn_api":
        for field in ("api", "context"):
            if field not in out:
                raise TargetFeatureManifestError(
                    f"feature {feature_id}: webnn_api browser probe must name {field}"
                )
    return out


def _feature_rows(data: dict[str, Any]) -> list[dict[str, Any]]:
    rows = data.get("feature")
    if not isinstance(rows, list) or not rows:
        raise TargetFeatureManifestError(
            "manifest must define at least one [[feature]]"
        )
    out: list[dict[str, str]] = []
    seen: set[str] = set()
    for raw in rows:
        if not isinstance(raw, dict):
            raise TargetFeatureManifestError("[[feature]] rows must be tables")
        feature_id = _require_str(raw, "id", context="feature")
        _check_id(feature_id, context="feature")
        if feature_id in seen:
            raise TargetFeatureManifestError(f"duplicate feature id {feature_id!r}")
        seen.add(feature_id)
        determinism = _require_str(raw, "determinism", context=f"feature {feature_id}")
        if determinism not in VALID_DETERMINISM:
            raise TargetFeatureManifestError(
                f"feature {feature_id}: determinism {determinism!r} must be one of "
                f"{sorted(VALID_DETERMINISM)}"
            )
        row = {
            "id": feature_id,
            "domain": _require_str(raw, "domain", context=f"feature {feature_id}"),
            "tier": _require_str(raw, "tier", context=f"feature {feature_id}"),
            "determinism": determinism,
            "description": _require_str(
                raw, "description", context=f"feature {feature_id}"
            ),
        }
        browser_probe = _browser_probe_row(feature_id, raw)
        if browser_probe is not None:
            row["browser_probe"] = browser_probe
        out.append(row)
    return out


def _target_rows(
    data: dict[str, Any],
    *,
    feature_index: dict[str, dict[str, Any]],
) -> list[dict[str, Any]]:
    rows = data.get("target")
    if not isinstance(rows, list) or not rows:
        raise TargetFeatureManifestError("manifest must define at least one [[target]]")

    out: list[dict[str, Any]] = []
    seen_targets: set[str] = set()
    for raw in rows:
        if not isinstance(raw, dict):
            raise TargetFeatureManifestError("[[target]] rows must be tables")
        target_id = _require_str(raw, "id", context="target")
        _check_id(target_id, context="target")
        if target_id in seen_targets:
            raise TargetFeatureManifestError(f"duplicate target id {target_id!r}")
        seen_targets.add(target_id)

        feature_rows = raw.get("feature")
        if not isinstance(feature_rows, list) or not feature_rows:
            raise TargetFeatureManifestError(
                f"target {target_id}: must define at least one [[target.feature]]"
            )
        target_features: list[dict[str, str]] = []
        seen_features: set[str] = set()
        for feature_raw in feature_rows:
            if not isinstance(feature_raw, dict):
                raise TargetFeatureManifestError(
                    f"target {target_id}: feature rows must be tables"
                )
            feature_id = _require_str(
                feature_raw, "id", context=f"target {target_id} feature"
            )
            if feature_id not in feature_index:
                raise TargetFeatureManifestError(
                    f"target {target_id}: unknown feature {feature_id!r}"
                )
            if feature_id in seen_features:
                raise TargetFeatureManifestError(
                    f"target {target_id}: duplicate feature {feature_id!r}"
                )
            seen_features.add(feature_id)
            support = _require_str(
                feature_raw,
                "support",
                context=f"target {target_id} feature {feature_id}",
            )
            if support not in VALID_SUPPORT:
                raise TargetFeatureManifestError(
                    f"target {target_id} feature {feature_id}: support {support!r} "
                    f"must be one of {sorted(VALID_SUPPORT)}"
                )
            determinism = feature_index[feature_id]["determinism"]
            if support == "baseline" and determinism in {
                "non_bit_exact",
                "observability",
                "device_dependent",
                "external_numeric",
                "external_provider",
                "host_capability",
            }:
                raise TargetFeatureManifestError(
                    f"target {target_id}: feature {feature_id!r} is {determinism} "
                    "and cannot be baseline"
                )
            gate = _require_str(
                feature_raw,
                "gate",
                context=f"target {target_id} feature {feature_id}",
            )
            fallback = _require_str(
                feature_raw,
                "fallback",
                context=f"target {target_id} feature {feature_id}",
            )
            target_features.append(
                {
                    "id": feature_id,
                    "support": support,
                    "gate": gate,
                    "fallback": fallback,
                    "unsupported_reason": _unsupported_reason(
                        target_id=target_id,
                        feature_id=feature_id,
                        support=support,
                        gate=gate,
                        fallback=fallback,
                    ),
                }
            )

        target_row: dict[str, Any] = {
            "id": target_id,
            "display_name": _require_str(
                raw, "display_name", context=f"target {target_id}"
            ),
            "family": _require_str(raw, "family", context=f"target {target_id}"),
            "target_triple": _require_str(
                raw, "target_triple", context=f"target {target_id}"
            ),
            "artifact_kind": _require_str(
                raw, "artifact_kind", context=f"target {target_id}"
            ),
            "description": _require_str(
                raw, "description", context=f"target {target_id}"
            ),
            "features": target_features,
        }
        browser_default = _optional_bool(
            raw,
            "browser_default",
            context=f"target {target_id}",
        )
        if browser_default is not None:
            target_row["browser_default"] = browser_default
        browser_host_imports = _optional_str_list_table(
            raw,
            "browser_host_imports",
            context=f"target {target_id}",
        )
        if browser_host_imports is not None:
            target_row["browser_host_imports"] = browser_host_imports
        out.append(target_row)
    return out


def _support(target: dict[str, Any], feature_id: str) -> str | None:
    for feature in target["features"]:
        if feature["id"] == feature_id:
            return feature["support"]
    return None


def _unsupported_reason(
    *,
    target_id: str,
    feature_id: str,
    support: str,
    gate: str,
    fallback: str,
) -> str:
    if support == "baseline":
        return (
            f"{feature_id} is baseline for {target_id}; failing {gate} "
            "makes the target profile invalid."
        )
    if support == "excluded":
        return (
            f"{feature_id} is excluded from {target_id}; use fallback "
            f"{fallback}."
        )
    return (
        f"{feature_id} for {target_id} requires {gate}; use fallback "
        f"{fallback} when the gate is not proven."
    )


def _feature_ids(target: dict[str, Any]) -> set[str]:
    return {feature["id"] for feature in target["features"]}


def _require_features(
    target: dict[str, Any],
    required: tuple[str, ...],
    *,
    reason: str,
) -> None:
    missing = sorted(set(required) - _feature_ids(target))
    if missing:
        raise TargetFeatureManifestError(
            f"target {target['id']}: missing {reason} features {missing}"
        )


def _validate_feature_contracts(features: list[dict[str, Any]]) -> None:
    by_id = {feature["id"]: feature for feature in features}

    for feature_id in WEBGPU_FEATURE_IDS:
        probe = by_id[feature_id].get("browser_probe")
        if not isinstance(probe, dict):
            raise TargetFeatureManifestError(
                f"feature {feature_id}: WebGPU browser probe metadata is required"
            )
    if by_id["webgpu.api"]["browser_probe"]["kind"] != "webgpu_api":
        raise TargetFeatureManifestError("webgpu.api must own the WebGPU API probe")
    if (
        by_id["webgpu.f16"]["browser_probe"].get("adapter_feature")
        != "shader-f16"
    ):
        raise TargetFeatureManifestError(
            "webgpu.f16 must own the shader-f16 adapter probe"
        )
    if (
        by_id["webgpu.subgroups"]["browser_probe"].get("adapter_feature")
        != "subgroups"
    ):
        raise TargetFeatureManifestError(
            "webgpu.subgroups must own the subgroups adapter probe"
        )
    if (
        by_id["webgpu.timestamp_queries"]["browser_probe"].get("adapter_feature")
        != "timestamp-query"
    ):
        raise TargetFeatureManifestError(
            "webgpu.timestamp_queries must own the timestamp-query adapter probe"
        )
    if by_id["webgpu.timestamp_queries"]["browser_probe"].get("observability") is not True:
        raise TargetFeatureManifestError(
            "webgpu.timestamp_queries must be marked observability-only"
        )

    webnn_probe = by_id["webnn.graph"].get("browser_probe")
    if not isinstance(webnn_probe, dict) or webnn_probe.get("kind") != "webnn_api":
        raise TargetFeatureManifestError("webnn.graph must own the WebNN API probe")


def _validate_target_contracts(targets: list[dict[str, Any]]) -> None:
    ids = tuple(target["id"] for target in targets)
    if ids != EXPECTED_TARGET_IDS:
        raise TargetFeatureManifestError(
            "target rows must exactly match doc 71 Immediate Concrete Work order: "
            f"{list(EXPECTED_TARGET_IDS)}; got {list(ids)}"
        )

    by_id = {target["id"]: target for target in targets}
    native = by_id["native"]
    _require_features(
        native,
        (
            "native.scalar",
            "native.vector",
            "native.threads",
            *NUMERIC_LIBRARY_FEATURE_IDS,
            "tensor.typed_strided_storage",
        ),
        reason="native target-feature",
    )

    for target in targets:
        if target["family"].startswith("wasm"):
            _require_features(
                target,
                WASM_PROFILE_FEATURE_IDS,
                reason="doc 71 WASM profile envelope",
            )

    browser = by_id["wasm-browser"]
    _require_features(
        browser,
        (*WEBGPU_FEATURE_IDS, "webnn.graph"),
        reason="browser accelerator exclusion",
    )
    if _support(browser, "webgpu.api") != "excluded":
        raise TargetFeatureManifestError(
            "wasm-browser must exclude webgpu.api; use wasm-browser-webgpu"
        )
    for feature_id in WEBGPU_FEATURE_IDS:
        if _support(browser, feature_id) != "excluded":
            raise TargetFeatureManifestError(
                f"wasm-browser must exclude {feature_id}; use wasm-browser-webgpu"
            )
    if _support(browser, "webnn.graph") != "excluded":
        raise TargetFeatureManifestError(
            "wasm-browser must exclude webnn.graph; use wasm-browser-webnn"
        )

    webgpu = by_id["wasm-browser-webgpu"]
    _require_features(
        webgpu,
        (*WEBGPU_FEATURE_IDS, "webnn.graph"),
        reason="WebGPU browser profile",
    )
    if _support(webgpu, "webgpu.api") != "gated":
        raise TargetFeatureManifestError("wasm-browser-webgpu must gate webgpu.api")
    for feature_id in WEBGPU_FEATURE_IDS:
        if _support(webgpu, feature_id) not in {"gated", "tracked"}:
            raise TargetFeatureManifestError(
                f"wasm-browser-webgpu must gate or track {feature_id}"
            )
    if _support(webgpu, "webnn.graph") != "excluded":
        raise TargetFeatureManifestError(
            "wasm-browser-webgpu must exclude WebNN graph dispatch"
        )

    webnn = by_id["wasm-browser-webnn"]
    _require_features(
        webnn,
        (*WEBGPU_FEATURE_IDS, "webnn.graph"),
        reason="WebNN browser profile",
    )
    if _support(webnn, "webnn.graph") != "gated":
        raise TargetFeatureManifestError("wasm-browser-webnn must gate webnn.graph")
    if _support(webnn, "webgpu.api") != "excluded":
        raise TargetFeatureManifestError(
            "wasm-browser-webnn must exclude WebGPU dispatch"
        )
    for feature_id in WEBGPU_FEATURE_IDS:
        if _support(webnn, feature_id) != "excluded":
            raise TargetFeatureManifestError(
                f"wasm-browser-webnn must exclude {feature_id}"
            )

    _validate_browser_host_imports(targets)

    for target in targets:
        if _support(target, "tensor.typed_strided_storage") != "baseline":
            raise TargetFeatureManifestError(
                f"target {target['id']}: typed strided storage must be baseline"
            )
        relaxed = _support(target, "wasm.relaxed_simd")
        if relaxed == "baseline":
            raise TargetFeatureManifestError(
                f"target {target['id']}: relaxed SIMD cannot be baseline"
            )


def _webgpu_host_imports(target: dict[str, Any]) -> list[str] | None:
    imports = target.get("browser_host_imports")
    if not isinstance(imports, dict):
        return None
    values = imports.get(WEBGPU_HOST_IMPORT_KIND)
    return values if isinstance(values, list) else None


def _validate_browser_host_imports(targets: list[dict[str, Any]]) -> None:
    by_id = {target["id"]: target for target in targets}
    browser_profiles = [
        target for target in targets if target["family"] == BROWSER_TARGET_FAMILY
    ]
    browser_defaults = [
        target["id"] for target in browser_profiles if target.get("browser_default")
    ]
    if browser_defaults != [BROWSER_TARGET_FAMILY]:
        raise TargetFeatureManifestError(
            f"exactly {BROWSER_TARGET_FAMILY} must be the default browser target profile"
        )
    for target in browser_profiles:
        if _webgpu_host_imports(target) is None:
            raise TargetFeatureManifestError(
                f"target {target['id']}: browser target must declare "
                f"browser_host_imports.{WEBGPU_HOST_IMPORT_KIND} as a list"
            )
    if _webgpu_host_imports(by_id[BROWSER_TARGET_FAMILY]) != []:
        raise TargetFeatureManifestError(
            f"{BROWSER_TARGET_FAMILY} must not require the WebGPU browser host import"
        )
    webgpu_selector = _webgpu_host_imports(by_id[WEBGPU_DISPATCH_PROFILE_ID])
    if not webgpu_selector or len(webgpu_selector) != 1:
        raise TargetFeatureManifestError(
            f"{WEBGPU_DISPATCH_PROFILE_ID} must be selected by exactly one WebGPU "
            "browser host import"
        )
    if _webgpu_host_imports(by_id["wasm-browser-webnn"]) != []:
        raise TargetFeatureManifestError(
            "wasm-browser-webnn must not require the WebGPU browser host import"
        )


def _derived_constants(targets: list[dict[str, Any]]) -> dict[str, str]:
    """Single-source browser/target-feature constants derived from the manifest."""

    by_id = {target["id"]: target for target in targets}
    webgpu_selector = _webgpu_host_imports(by_id[WEBGPU_DISPATCH_PROFILE_ID])
    assert webgpu_selector is not None and len(webgpu_selector) == 1
    return {
        "WEBGPU_DISPATCH_HOST_IMPORT": webgpu_selector[0],
        "TARGET_FEATURE_MANIFEST_ASSET_NAME": TARGET_FEATURE_MANIFEST_ASSET_NAME,
        "BROWSER_TARGET_FAMILY": BROWSER_TARGET_FAMILY,
    }


def load_source(path: Path = SOURCE_TOML) -> dict[str, Any]:
    if not path.is_file():
        raise TargetFeatureManifestError(f"missing source manifest: {path}")
    with path.open("rb") as handle:
        return tomllib.load(handle)


def build_model(data: dict[str, Any] | None = None) -> dict[str, Any]:
    raw = copy.deepcopy(load_source() if data is None else data)
    if raw.get("schema_version") != 1:
        raise TargetFeatureManifestError("schema_version must be 1")
    meta = raw.get("meta")
    if not isinstance(meta, dict):
        raise TargetFeatureManifestError("manifest must define [meta]")

    features = _feature_rows(raw)
    _validate_feature_contracts(features)
    feature_index = {feature["id"]: feature for feature in features}
    targets = _target_rows(raw, feature_index=feature_index)
    _validate_target_contracts(targets)

    return {
        "schema_version": 1,
        "meta": {
            "source": SOURCE_TOML.relative_to(ROOT).as_posix(),
            "source_doc": _require_str(meta, "source_doc", context="meta"),
            "authority": _require_str(meta, "authority", context="meta"),
            "description": _require_str(meta, "description", context="meta"),
            "generated_by": "tools/gen_target_feature_manifest.py",
        },
        "constants": _derived_constants(targets),
        "features": features,
        "targets": targets,
    }


def render_json(model: dict[str, Any]) -> str:
    return json.dumps(model, indent=2, sort_keys=True) + "\n"


def render_python(model: dict[str, Any]) -> str:
    manifest_json = render_json(model)
    return (
        "# @generated by tools/gen_target_feature_manifest.py from\n"
        "# src/molt/target_feature_manifest.toml. DO NOT EDIT.\n"
        "\n"
        "from __future__ import annotations\n"
        "\n"
        "import copy\n"
        "import json\n"
        "from typing import Any\n"
        "\n"
        f"_MANIFEST_JSON = r'''{manifest_json}'''\n"
        "MANIFEST: dict[str, Any] = json.loads(_MANIFEST_JSON)\n"
        "TARGET_PROFILE_IDS: tuple[str, ...] = tuple(\n"
        "    target['id'] for target in MANIFEST['targets']\n"
        ")\n"
        "FEATURE_IDS: tuple[str, ...] = tuple(\n"
        "    feature['id'] for feature in MANIFEST['features']\n"
        ")\n"
        "CONSTANTS: dict[str, str] = dict(MANIFEST['constants'])\n"
        "WEBGPU_DISPATCH_HOST_IMPORT: str = CONSTANTS['WEBGPU_DISPATCH_HOST_IMPORT']\n"
        "TARGET_FEATURE_MANIFEST_ASSET_NAME: str = "
        "CONSTANTS['TARGET_FEATURE_MANIFEST_ASSET_NAME']\n"
        "BROWSER_TARGET_FAMILY: str = CONSTANTS['BROWSER_TARGET_FAMILY']\n"
        "\n"
        "\n"
        "def target_feature_constants() -> dict[str, str]:\n"
        "    return dict(CONSTANTS)\n"
        "\n"
        "\n"
        "def target_feature_manifest() -> dict[str, Any]:\n"
        "    return copy.deepcopy(MANIFEST)\n"
        "\n"
        "\n"
        "def target_profile_ids() -> tuple[str, ...]:\n"
        "    return TARGET_PROFILE_IDS\n"
        "\n"
        "\n"
        "def target_profile(profile_id: str) -> dict[str, Any]:\n"
        "    for target in MANIFEST['targets']:\n"
        "        if target['id'] == profile_id:\n"
        "            return copy.deepcopy(target)\n"
        "    raise KeyError(f'unknown target profile {profile_id!r}')\n"
        "\n"
        "\n"
        "def target_feature_row(profile_id: str, feature_id: str) -> dict[str, str] | None:\n"
        "    target = target_profile(profile_id)\n"
        "    for feature in target['features']:\n"
        "        if feature['id'] == feature_id:\n"
        "            return copy.deepcopy(feature)\n"
        "    return None\n"
        "\n"
        "\n"
        "def target_feature_support(profile_id: str, feature_id: str) -> str | None:\n"
        "    row = target_feature_row(profile_id, feature_id)\n"
        "    return None if row is None else row['support']\n"
    )


def render_js(model: dict[str, Any]) -> str:
    """Emit the single-source browser target-feature constants for JS consumers."""

    constants = model["constants"]
    lines = [
        "// @generated by tools/gen_target_feature_manifest.py from",
        "// src/molt/target_feature_manifest.toml. DO NOT EDIT.",
        "",
    ]
    for name in sorted(constants):
        lines.append(f"export const {name} = {json.dumps(constants[name])};")
    lines.append("")
    return "\n".join(lines)


def _check_output(path: Path, expected: str) -> bool:
    if not path.exists():
        print(f"MISSING generated file: {path.relative_to(ROOT)}", file=sys.stderr)
        return False
    current = path.read_text(encoding="utf-8")
    if current != expected:
        print(
            f"STALE generated file: {path.relative_to(ROOT)}\n"
            "  run `python3 tools/gen_target_feature_manifest.py`",
            file=sys.stderr,
        )
        return False
    return True


def main(argv: list[str]) -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument(
        "--check",
        action="store_true",
        help="exit 1 if generated artifacts are stale; do not write",
    )
    args = parser.parse_args(argv)

    try:
        model = build_model()
        py_text = render_python(model)
        json_text = render_json(model)
        js_text = render_js(model)
    except TargetFeatureManifestError as exc:
        print(f"target feature manifest generation FAILED:\n  {exc}", file=sys.stderr)
        return 1

    if args.check:
        ok = (
            _check_output(OUT_PY, py_text)
            and _check_output(OUT_JSON, json_text)
            and _check_output(OUT_JS, js_text)
        )
        if ok:
            print("target feature manifest: in sync")
        return 0 if ok else 1

    OUT_PY.write_text(py_text, encoding="utf-8", newline="\n")
    OUT_JSON.write_text(json_text, encoding="utf-8", newline="\n")
    OUT_JS.write_text(js_text, encoding="utf-8", newline="\n")
    print(f"wrote {OUT_PY.relative_to(ROOT)}")
    print(f"wrote {OUT_JSON.relative_to(ROOT)}")
    print(f"wrote {OUT_JS.relative_to(ROOT)}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main(sys.argv[1:]))
