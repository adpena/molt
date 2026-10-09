"""Runtime tree-shake and split-app optimization policy authority."""

from __future__ import annotations

from collections.abc import Mapping, Sequence
import hashlib
from pathlib import Path
import os
import sys
from molt.temporary_artifacts import OwnedTemporaryDirectory
import time

from molt._wasm_runtime_exports import wasm_split_runtime_export_name_for_import
from molt.cli.wasm_link_cache import (
    WasmLinkCacheEntry,
    _default_wasm_link_cache,
    _invalidate_wasm_link_cache_entry,
    _locked_wasm_link_cache_entry,
    _publish_wasm_link_cache_entry,
    _read_wasm_link_cache_entry,
    _wasm_link_cache_entry,
)
from molt.cli.python_source_closure import local_python_import_closure
from molt.exact_json import canonical_json_sha256
from molt.wasm_optimization import wasm_link_policy
from molt.wasm_optimizer_identity import (
    WasmOptimizerExecutableIdentity,
    WasmOptimizerIdentityError,
    build_wasm_optimizer_attestation,
    validate_wasm_optimizer_attestation,
)
from wasm_link_format import (
    _ESSENTIAL_EXPORTS,
    _write_string,
    _write_varuint,
)
from wasm_link_fact_provider import WasmFactsProvider
from wasm_link_operations import build_sections, parse_sections
from wasm_link_optimize import (
    _post_link_optimize,
    _strip_unused_module_function_imports,
)
from wasm_optimize import optimize as optimize_wasm

TOOLS_ROOT = Path(__file__).resolve().parent

_TREE_SHAKE_RUNTIME_CACHE_SCHEMA = "runtime-tree-shake-v8"
_SPLIT_APP_OPTIMIZE_CACHE_SCHEMA = "split-app-optimize-v7"
_WASM_LINK_CACHE_METRIC_SUFFIXES = (
    "requests",
    "hits",
    "misses",
    "corruptions",
    "bytes_read",
    "bytes_written",
    "lock_wait_ms",
    "lookup_ms",
    "publish_ms",
    "wall_ms",
    "publish_errors",
)
_WASM_OPT_CACHE_METRIC_SUFFIXES = (
    "optimizer_wall_ms",
    "optimizer_peak_rss_kb",
    "optimizer_peak_total_rss_kb",
    "timeouts",
    "failures",
    "identity_errors",
)


def _wasm_link_cache_root() -> Path:
    return _default_wasm_link_cache()


def _empty_wasm_link_cache_metrics() -> dict[str, int | float]:
    metrics = {
        f"{prefix}_{suffix}": 0
        for prefix in ("runtime_tree_shake_cache", "split_app_optimize_cache")
        for suffix in _WASM_LINK_CACHE_METRIC_SUFFIXES
    }
    metrics.update(
        {
            f"split_app_optimize_cache_{suffix}": 0
            for suffix in _WASM_OPT_CACHE_METRIC_SUFFIXES
        }
    )
    return metrics


def _cache_metric_add(
    metrics: dict[str, int | float] | None,
    name: str,
    value: int | float,
) -> None:
    if metrics is not None:
        metrics[name] = round(float(metrics.get(name, 0)) + float(value), 6)


def _cache_metric_max(
    metrics: dict[str, int | float] | None,
    name: str,
    value: int | float | None,
) -> None:
    if metrics is not None and value is not None:
        metrics[name] = max(float(metrics.get(name, 0)), float(value))


def _record_wasm_opt_telemetry_cache_metrics(
    metrics: dict[str, int | float] | None,
    prefix: str,
    telemetry: Mapping[str, object],
) -> None:
    wall_ms = telemetry.get("wasm_opt_wall_ms")
    if isinstance(wall_ms, (int, float)):
        _cache_metric_add(metrics, f"{prefix}_optimizer_wall_ms", wall_ms)
    for suffix in ("peak_rss_kb", "peak_total_rss_kb"):
        value = telemetry.get(f"wasm_opt_{suffix}")
        if isinstance(value, (int, float)):
            _cache_metric_max(metrics, f"{prefix}_optimizer_{suffix}", value)
    status = telemetry.get("status")
    if status == "timeout":
        _cache_metric_add(metrics, f"{prefix}_timeouts", 1)
    elif telemetry.get("ok") is False:
        _cache_metric_add(metrics, f"{prefix}_failures", 1)
        if status == "identity-error":
            _cache_metric_add(metrics, f"{prefix}_identity_errors", 1)


def _publish_wasm_link_cache_result(
    entry: WasmLinkCacheEntry,
    data: bytes,
    *,
    metrics: dict[str, int | float] | None,
    metric_prefix: str,
    label: str,
    payload: Mapping[str, object] | None = None,
) -> None:
    publish_started = time.perf_counter()
    try:
        _publish_wasm_link_cache_entry(entry, data, payload=payload)
    except OSError as exc:
        _cache_metric_add(metrics, f"{metric_prefix}_publish_errors", 1)
        print(f"{label} cache publication failed: {exc}", file=sys.stderr)
    else:
        _cache_metric_add(metrics, f"{metric_prefix}_bytes_written", len(data))
    _cache_metric_add(
        metrics,
        f"{metric_prefix}_publish_ms",
        (time.perf_counter() - publish_started) * 1000.0,
    )


def _wasm_link_cache_authority_digest(*, repo_root: Path | None = None) -> str:
    root = (repo_root or TOOLS_ROOT.parent).resolve(strict=True)
    return local_python_import_closure(
        root, (root / "tools" / "wasm_link.py",)
    ).content_digest


def _wasm_facts_cache_authority_digest(
    facts_provider: WasmFactsProvider,
) -> str:
    provider_identity = facts_provider.authority_digest
    if len(provider_identity) != 64 or any(
        character not in "0123456789abcdef" for character in provider_identity
    ):
        raise ValueError("WASM facts provider authority must be a lowercase SHA-256")
    # The provider identity binds the scanner bytes and wire schema. Each
    # caller already keys on the exact input bytes; scanning them again before
    # a cache lookup adds no identity and defeats cross-process warm reuse.
    return provider_identity


def _wasm_link_cache_key(schema: str, components: Mapping[str, object]) -> str:
    """Hash named key components as canonical JSON, the final-link encoding.

    JSON keeps every component's boundary, so no two component sets share a
    key; bytes enter as their SHA-256.
    """
    return canonical_json_sha256({"schema": schema, "components": dict(components)})


def _report_wasm_link_cache_miss(
    label: str, status: str, key: str, components: Mapping[str, object]
) -> None:
    """Name each key component's digest, so two misses show what differs."""
    detail = ", ".join(
        f"{name} {canonical_json_sha256(value)[:12]}"
        for name, value in sorted(components.items())
    )
    print(f"{label} cache {status}: key {key[:16]} ({detail})", file=sys.stderr)


def _split_app_optimize_cache_components(
    *,
    app_data: bytes,
    reference_data: bytes | None,
    optimize: bool,
    optimize_level: str,
    contract_keep_set: set[str],
    facts_authority_digest: str,
    optimizer_identity: WasmOptimizerExecutableIdentity | None = None,
    preserve_debug: bool = False,
) -> dict[str, object] | None:
    if optimize and optimizer_identity is None:
        return None
    return {
        "app_sha256": hashlib.sha256(app_data).hexdigest(),
        "reference_sha256": None
        if reference_data is None
        else hashlib.sha256(reference_data).hexdigest(),
        "optimize": optimize,
        "optimize_level": optimize_level,
        "preserve_debug": preserve_debug,
        "exports": sorted(contract_keep_set),
        "optimizer": {
            "sha256": optimizer_identity.sha256,
            "binaryen_version": optimizer_identity.binaryen_version,
        }
        if optimize and optimizer_identity is not None
        else None,
        "tool": _wasm_link_cache_authority_digest(),
        "facts_authority": facts_authority_digest,
    }


def _split_app_optimize_cache_key(
    *,
    app_data: bytes,
    reference_data: bytes | None,
    optimize: bool,
    optimize_level: str,
    contract_keep_set: set[str],
    facts_authority_digest: str,
    optimizer_identity: WasmOptimizerExecutableIdentity | None = None,
    preserve_debug: bool = False,
) -> str | None:
    components = _split_app_optimize_cache_components(
        app_data=app_data,
        reference_data=reference_data,
        optimize=optimize,
        optimize_level=optimize_level,
        contract_keep_set=contract_keep_set,
        facts_authority_digest=facts_authority_digest,
        optimizer_identity=optimizer_identity,
        preserve_debug=preserve_debug,
    )
    if components is None:
        return None
    return _wasm_link_cache_key(_SPLIT_APP_OPTIMIZE_CACHE_SCHEMA, components)


def _tree_shake_runtime_cache_components(
    *,
    runtime_data: bytes,
    normalized_required_exports: set[str],
    facts_authority_digest: str,
    preserve_debug: bool = False,
) -> dict[str, object]:
    return {
        "runtime_sha256": hashlib.sha256(runtime_data).hexdigest(),
        "exports": sorted(normalized_required_exports),
        "preserve_debug": preserve_debug,
        "tool": _wasm_link_cache_authority_digest(),
        "facts_authority": facts_authority_digest,
    }


def _tree_shake_runtime_cache_key(
    *,
    runtime_data: bytes,
    normalized_required_exports: set[str],
    facts_authority_digest: str,
    preserve_debug: bool = False,
) -> str:
    return _wasm_link_cache_key(
        _TREE_SHAKE_RUNTIME_CACHE_SCHEMA,
        _tree_shake_runtime_cache_components(
            runtime_data=runtime_data,
            normalized_required_exports=normalized_required_exports,
            facts_authority_digest=facts_authority_digest,
            preserve_debug=preserve_debug,
        ),
    )


def _transform_tree_shake_runtime(
    runtime_data: bytes,
    *,
    normalized_required_exports: set[str],
    facts_provider: WasmFactsProvider,
    preserve_debug: bool = False,
) -> tuple[bytes, int, int]:
    """Apply the deterministic runtime export filter and structural cleanup.

    Cargo/LLVM runtime generation owns shared-runtime body optimization and any
    future whole-runtime Binaryen pass. The app-link stage must not run a second
    whole-runtime optimizer pipeline: that duplicates expensive work and can
    delete the app-independent public ABI.
    """

    exports = facts_provider(runtime_data).exports.values()
    filtered = [
        (export.name, export.kind, export.index)
        for export in exports
        if export.kind != 0 or export.name in normalized_required_exports
    ]
    kept_exports = len(filtered)
    stripped_exports = len(exports) - kept_exports
    sections = parse_sections(runtime_data)
    new_sections: list[tuple[int, bytes]] = []

    for section_id, payload in sections:
        if section_id != 7:
            new_sections.append((section_id, payload))
            continue

        new_payload = bytearray(_write_varuint(len(filtered)))
        for name, kind, index in filtered:
            new_payload.extend(_write_string(name))
            new_payload.append(kind)
            new_payload.extend(_write_varuint(index))
        new_sections.append((7, bytes(new_payload)))

    print(
        f"Runtime tree-shake: kept {kept_exports} exports, "
        f"stripped {stripped_exports} unused function exports",
        file=sys.stderr,
    )
    stripped_data = build_sections(new_sections)
    optimized = _post_link_optimize(
        stripped_data,
        preserve_exports=normalized_required_exports,
        preserve_debug=preserve_debug,
        facts_provider=facts_provider,
    )
    if len(optimized) != len(stripped_data):
        print(
            f"Runtime post-link optimize: {len(stripped_data):,} -> "
            f"{len(optimized):,} bytes "
            f"({len(stripped_data) - len(optimized):,} bytes eliminated)",
            file=sys.stderr,
        )
    return optimized, kept_exports, stripped_exports


def _tree_shake_runtime(
    runtime_data: bytes,
    required_exports: set[str],
    *,
    facts_provider: WasmFactsProvider,
    operation_counts: dict[str, int | float] | None = None,
    preserve_debug: bool = False,
) -> bytes:
    """Filter one runtime once per cache key under the cache's single-flight lock."""

    # Canonicalize app imports to the runtime export naming convention before
    # filtering. App imports use unprefixed ABI names while the shared runtime
    # exports the corresponding ``molt_*`` symbols.
    normalized_required_exports = set(required_exports)
    for name in required_exports:
        export_name = wasm_split_runtime_export_name_for_import(name)
        if export_name is not None:
            normalized_required_exports.add(export_name)
    # Host-facing publication roots have one generated authority in
    # ``output_export_policy.essential_exports``. Do not recreate a local list.
    normalized_required_exports.update(_ESSENTIAL_EXPORTS)
    raw_dynamic_exports = os.environ.get(
        "MOLT_WASM_DYNAMIC_REQUIRED_EXPORTS", ""
    ).strip()
    if raw_dynamic_exports:
        normalized_required_exports.update(
            name.strip() for name in raw_dynamic_exports.split(",") if name.strip()
        )

    cache_started = time.perf_counter()
    metric_prefix = "runtime_tree_shake_cache"
    _cache_metric_add(operation_counts, f"{metric_prefix}_requests", 1)
    facts_authority_digest = _wasm_facts_cache_authority_digest(
        facts_provider,
    )
    cache_components = _tree_shake_runtime_cache_components(
        runtime_data=runtime_data,
        normalized_required_exports=normalized_required_exports,
        facts_authority_digest=facts_authority_digest,
        preserve_debug=preserve_debug,
    )
    cache_key = _wasm_link_cache_key(_TREE_SHAKE_RUNTIME_CACHE_SCHEMA, cache_components)
    cache_entry = _wasm_link_cache_entry(
        "runtime_tree_shake",
        _TREE_SHAKE_RUNTIME_CACHE_SCHEMA,
        cache_key,
        cache_root=_wasm_link_cache_root(),
    )
    with _locked_wasm_link_cache_entry(cache_entry) as lock_wait_ms:
        _cache_metric_add(
            operation_counts, f"{metric_prefix}_lock_wait_ms", lock_wait_ms
        )
        lookup_started = time.perf_counter()
        cached = _read_wasm_link_cache_entry(cache_entry)
        _cache_metric_add(
            operation_counts,
            f"{metric_prefix}_lookup_ms",
            (time.perf_counter() - lookup_started) * 1000.0,
        )
        if cached.data is not None:
            _cache_metric_add(operation_counts, f"{metric_prefix}_hits", 1)
            _cache_metric_add(
                operation_counts, f"{metric_prefix}_bytes_read", cached.bytes_read
            )
            _cache_metric_add(
                operation_counts,
                f"{metric_prefix}_wall_ms",
                (time.perf_counter() - cache_started) * 1000.0,
            )
            print(f"Runtime tree-shake cache hit: {cache_entry.root}", file=sys.stderr)
            return cached.data

        _cache_metric_add(operation_counts, f"{metric_prefix}_misses", 1)
        _report_wasm_link_cache_miss(
            "Runtime tree-shake", cached.status, cache_key, cache_components
        )
        if cached.status == "corrupt":
            _cache_metric_add(operation_counts, f"{metric_prefix}_corruptions", 1)
            _invalidate_wasm_link_cache_entry(cache_entry)

        optimized, kept_exports, stripped_exports = _transform_tree_shake_runtime(
            runtime_data,
            normalized_required_exports=normalized_required_exports,
            facts_provider=facts_provider,
            preserve_debug=preserve_debug,
        )
        _publish_wasm_link_cache_result(
            cache_entry,
            optimized,
            metrics=operation_counts,
            metric_prefix=metric_prefix,
            label="Runtime structural optimize",
            payload={
                "result_kind": "runtime-generation-plus-structural-link-cleanup",
                "kept_exports": kept_exports,
                "stripped_exports": stripped_exports,
            },
        )
        _cache_metric_add(
            operation_counts,
            f"{metric_prefix}_wall_ms",
            (time.perf_counter() - cache_started) * 1000.0,
        )
        return optimized


def _optimize_split_app_module(
    app_data: bytes,
    *,
    reference_data: bytes | None,
    optimize: bool,
    optimize_level: str,
    contract_keep_set: set[str],
    attestation: dict[str, object] | None = None,
    telemetry: dict[str, object] | None = None,
    operation_counts: dict[str, int | float] | None = None,
    facts_provider: WasmFactsProvider,
    optimizer_identity: WasmOptimizerExecutableIdentity | None = None,
    preserve_debug: bool = False,
) -> bytes:
    """Deforest the split-runtime app artifact without collapsing its imports.

    The split app module must remain unlinked so it can continue importing the
    deploy runtime, but it still benefits from the same post-link cleanup passes
    as the fully linked artifact. Apply those cleanup passes first, then run
    wasm-opt when requested.
    """
    if operation_counts is not None:
        operation_counts["split_app_optimize_requests"] = 1
    cache_started = time.perf_counter()
    metric_prefix = "split_app_optimize_cache"
    active_telemetry = telemetry if telemetry is not None else {}
    _cache_metric_add(operation_counts, f"{metric_prefix}_requests", 1)
    if optimize and optimizer_identity is None:
        _cache_metric_add(operation_counts, f"{metric_prefix}_identity_errors", 1)
        _cache_metric_add(
            operation_counts,
            f"{metric_prefix}_wall_ms",
            (time.perf_counter() - cache_started) * 1000.0,
        )
        raise RuntimeError(
            "required split-app wasm optimization has no invocation-scoped "
            "executable identity"
        )
    facts_authority_digest = _wasm_facts_cache_authority_digest(
        facts_provider,
    )
    cache_components = _split_app_optimize_cache_components(
        app_data=app_data,
        reference_data=reference_data,
        optimize=optimize,
        optimize_level=optimize_level,
        contract_keep_set=contract_keep_set,
        facts_authority_digest=facts_authority_digest,
        optimizer_identity=optimizer_identity,
        preserve_debug=preserve_debug,
    )
    assert cache_components is not None
    cache_key = _wasm_link_cache_key(_SPLIT_APP_OPTIMIZE_CACHE_SCHEMA, cache_components)
    cache_entry = _wasm_link_cache_entry(
        "split_app_optimize",
        _SPLIT_APP_OPTIMIZE_CACHE_SCHEMA,
        cache_key,
        cache_root=_wasm_link_cache_root(),
    )
    with _locked_wasm_link_cache_entry(cache_entry) as lock_wait_ms:
        _cache_metric_add(
            operation_counts, f"{metric_prefix}_lock_wait_ms", lock_wait_ms
        )
        lookup_started = time.perf_counter()
        cached = _read_wasm_link_cache_entry(cache_entry)
        _cache_metric_add(
            operation_counts,
            f"{metric_prefix}_lookup_ms",
            (time.perf_counter() - lookup_started) * 1000.0,
        )
        cached_payload = dict(cached.payload or {})
        cache_identity_matches = cached.data is not None
        if cache_identity_matches and optimize:
            assert optimizer_identity is not None
            try:
                cached_payload = validate_wasm_optimizer_attestation(cached_payload)
            except WasmOptimizerIdentityError:
                cache_identity_matches = False
            else:
                cached_artifact_sha256 = hashlib.sha256(cached.data).hexdigest()
                cache_identity_matches = (
                    cached_payload.get("wasm_opt_sha256") == optimizer_identity.sha256
                    and cached_payload.get("binaryen_version")
                    == optimizer_identity.binaryen_version
                    and cached_payload.get("optimizer_output_sha256")
                    == cached_artifact_sha256
                    and cached_payload.get("published_output_sha256")
                    == cached_artifact_sha256
                )
        if cached.data is not None and cache_identity_matches:
            _cache_metric_add(operation_counts, f"{metric_prefix}_hits", 1)
            _cache_metric_add(
                operation_counts, f"{metric_prefix}_bytes_read", cached.bytes_read
            )
            if attestation is not None and optimize:
                attestation.clear()
                attestation.update(cached_payload)
            if optimize:
                assert optimizer_identity is not None
                active_telemetry.update(
                    {
                        "cache_hit": True,
                        "wasm_opt_path": str(optimizer_identity.path),
                    }
                )
            _cache_metric_add(
                operation_counts,
                f"{metric_prefix}_wall_ms",
                (time.perf_counter() - cache_started) * 1000.0,
            )
            return cached.data
        _cache_metric_add(operation_counts, f"{metric_prefix}_misses", 1)
        _report_wasm_link_cache_miss(
            "Split app optimize",
            "stale-optimizer" if cached.data is not None else cached.status,
            cache_key,
            cache_components,
        )
        if cached.status == "corrupt" or cached.data is not None:
            _cache_metric_add(operation_counts, f"{metric_prefix}_corruptions", 1)
            _invalidate_wasm_link_cache_entry(cache_entry)

        optimized = _post_link_optimize(
            app_data,
            reference_data=reference_data,
            preserve_exports=contract_keep_set,
            preserve_reference_exports=False,
            preserve_debug=preserve_debug,
            facts_provider=facts_provider,
        )
        stripped = _strip_unused_module_function_imports(
            optimized,
            module_name="molt_runtime",
            facts_provider=facts_provider,
        )
        if stripped is not None:
            optimized = stripped
        result = optimized
        active_attestation = attestation if attestation is not None else {}
        if optimize:
            assert optimizer_identity is not None
            optimizer_policy = wasm_link_policy(
                optimize_level, preserve_debug=preserve_debug
            )
            with OwnedTemporaryDirectory(prefix="molt-split-app-opt-") as tmp:
                app_path = Path(tmp) / "app_split_preopt.wasm"
                app_path.write_bytes(optimized)
                active_attestation.update(
                    {
                        "optimization_level": optimizer_policy.level,
                        "optimization_converge": optimizer_policy.converge,
                        "optimization_apply_level": optimizer_policy.apply_level,
                        "optimization_preserve_debug": preserve_debug,
                        "optimization_extra_passes": list(
                            optimizer_policy.extra_passes
                        ),
                        "optimizer_input_sha256": hashlib.sha256(optimized).hexdigest(),
                    }
                )
                required_function_exports = {
                    name
                    for name, fact in facts_provider(optimized).exports.items()
                    if fact.kind == 0
                } & contract_keep_set
                _cache_metric_add(operation_counts, "split_app_wasm_opt_runs", 1)
                optimizer_ok = _run_wasm_opt_via_optimize(
                    app_path,
                    level=optimizer_policy.level,
                    converge=optimizer_policy.converge,
                    required_exports=required_function_exports,
                    apply_level=optimizer_policy.apply_level,
                    extra_passes=optimizer_policy.extra_passes,
                    attestation=active_attestation,
                    telemetry=active_telemetry,
                    optimizer_identity=optimizer_identity,
                    preserve_debug=preserve_debug,
                )
                _record_wasm_opt_telemetry_cache_metrics(
                    operation_counts, metric_prefix, active_telemetry
                )
                if optimizer_ok:
                    result = app_path.read_bytes()
                    active_attestation["ok"] = True
                    active_attestation["optimizer_output_sha256"] = hashlib.sha256(
                        result
                    ).hexdigest()
                else:
                    failure = str(
                        active_telemetry.get("error", "unknown optimizer failure")
                    )
                    raise RuntimeError(
                        f"required split-app wasm optimization failed: {failure}"
                    )
        cache_payload: dict[str, object] = {}
        if optimize:
            assert optimizer_identity is not None
            cache_payload = build_wasm_optimizer_attestation(
                active_attestation,
                published_output=result,
            )
            if attestation is not None:
                attestation.clear()
                attestation.update(cache_payload)
            active_telemetry.update(
                {
                    "cache_hit": False,
                    "wasm_opt_path": str(optimizer_identity.path),
                }
            )
        _publish_wasm_link_cache_result(
            cache_entry,
            result,
            metrics=operation_counts,
            metric_prefix=metric_prefix,
            label="Split app optimize",
            payload=cache_payload,
        )
        _cache_metric_add(
            operation_counts,
            f"{metric_prefix}_wall_ms",
            (time.perf_counter() - cache_started) * 1000.0,
        )
        return result


def _run_wasm_opt_via_optimize(
    linked: Path,
    level: str = "Oz",
    *,
    converge: bool | None = None,
    required_exports: set[str],
    apply_level: bool | None = None,
    extra_passes: Sequence[str] | None = None,
    attestation: dict[str, object] | None = None,
    telemetry: dict[str, object] | None = None,
    optimizer_identity: WasmOptimizerExecutableIdentity | None = None,
    preserve_debug: bool = False,
) -> bool:
    """Run the canonical optimizer with separate provenance and telemetry."""

    policy = wasm_link_policy(level, preserve_debug=preserve_debug)
    resolved_converge = policy.converge if converge is None else converge
    resolved_apply_level = policy.apply_level if apply_level is None else apply_level
    resolved_extra_passes = (
        list(extra_passes) if extra_passes is not None else list(policy.extra_passes)
    )

    pre_size = linked.stat().st_size
    result = optimize_wasm(
        linked,
        output_path=linked,
        level=level,
        extra_passes=resolved_extra_passes,
        converge=resolved_converge,
        required_exports=required_exports,
        apply_level=resolved_apply_level,
        optimizer_identity=optimizer_identity,
        preserve_debug=preserve_debug,
    )

    if not result["ok"]:
        err = result.get("error", "unknown error")
        if telemetry is not None:
            telemetry.update(
                {
                    "ok": False,
                    "status": result.get("status", "failed"),
                    "error": err,
                    "pipeline": result.get("pipeline", []),
                    "wasm_opt_path": result.get("wasm_opt_path"),
                    "wasm_opt_sha256": result.get("wasm_opt_sha256"),
                    "wasm_opt_wall_ms": round(
                        float(result.get("elapsed_s", 0.0)) * 1000.0, 6
                    ),
                    "wasm_opt_peak_rss_kb": result.get("peak_rss_kb"),
                    "wasm_opt_peak_total_rss_kb": result.get("peak_total_rss_kb"),
                }
            )
        print(f"wasm-opt failed: {err}", file=sys.stderr)
        return False

    if optimizer_identity is not None and (
        result.get("wasm_opt_path") != str(optimizer_identity.path)
        or result.get("wasm_opt_sha256") != optimizer_identity.sha256
        or result.get("binaryen_version") != optimizer_identity.binaryen_version
    ):
        if telemetry is not None:
            telemetry.update(
                {
                    "ok": False,
                    "status": "identity-error",
                    "error": "wasm-opt crossed its invocation-scoped identity",
                    "wasm_opt_path": result.get("wasm_opt_path"),
                }
            )
        print(
            "wasm-opt crossed its invocation-scoped identity",
            file=sys.stderr,
        )
        return False

    before = result.get("before")
    after = result.get("after")
    if attestation is not None:
        attestation.update(
            {
                "ok": True,
                "status": result.get("status", "success"),
                "binaryen_version": result.get("binaryen_version", ""),
                "wasm_opt_sha256": result.get("wasm_opt_sha256"),
                "optimization_level": level,
                "optimization_converge": resolved_converge,
                "optimization_apply_level": resolved_apply_level,
                "optimization_preserve_debug": preserve_debug,
                "optimization_extra_passes": resolved_extra_passes,
                "pipeline": result.get("pipeline", []),
                "optimizer_input_sha256": (
                    before.get("sha256") if isinstance(before, Mapping) else None
                ),
                "optimizer_output_sha256": (
                    after.get("sha256") if isinstance(after, Mapping) else None
                ),
            }
        )
    if telemetry is not None:
        telemetry.update(
            {
                "ok": True,
                "status": result.get("status", "success"),
                "wasm_opt_path": result.get("wasm_opt_path"),
                "wasm_opt_wall_ms": round(
                    float(result.get("elapsed_s", 0.0)) * 1000.0, 6
                ),
                "wasm_opt_peak_rss_kb": result.get("peak_rss_kb"),
                "wasm_opt_peak_total_rss_kb": result.get("peak_total_rss_kb"),
            }
        )

    post_size = result["output_bytes"]
    savings = pre_size - post_size
    if savings > 0:
        print(
            f"wasm-opt ({level}): {savings:,} bytes saved "
            f"({savings / pre_size * 100:.1f}% reduction, "
            f"{post_size:,} bytes final)",
            file=sys.stderr,
        )
    return True
