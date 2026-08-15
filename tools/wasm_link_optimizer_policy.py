"""Runtime tree-shake and split-app optimization policy authority."""

from __future__ import annotations
from collections.abc import Callable, Mapping, Sequence
from pathlib import Path
import os
import sys
import tempfile
import time
from typing import Any

_API: Mapping[str, Any] | None = None


def configure_api(api: Mapping[str, Any]) -> None:
    global _API
    _API = api


def _api(name: str) -> Any:
    if _API is None:
        raise RuntimeError("WASM optimizer policy API is not configured")
    return _API[name]


def _tree_shake_runtime(
    runtime_data: bytes,
    required_exports: set[str],
    *,
    facts_provider: Callable[[bytes], dict[str, object]],
    operation_counts: dict[str, int | float] | None = None,
) -> bytes:
    """Strip unused exports from the runtime module and eliminate dead code.

    Rewrites the export section to only include functions in *required_exports*
    (plus memory/table/global exports which are always kept), then applies the
    linker's verified structural cleanup. Cargo/LLVM code generation owns the
    current shared-runtime body optimization. Any future Binaryen runtime pass
    belongs in runtime-generation custody; this app-link stage never runs a
    second whole-runtime Binaryen pipeline.
    """
    # Canonicalize the app import surface to the runtime export naming
    # convention.  The app imports the unprefixed ABI names (e.g. `alloc`,
    # `module_import`), while the runtime exports the corresponding
    # `molt_*` symbols.  Without this normalization, split-runtime
    # tree-shaking strips every function export even when the app has a
    # large live runtime dependency surface.
    normalized_required_exports = set(required_exports)
    for name in required_exports:
        export_name = _api("wasm_split_runtime_export_name_for_import")(name)
        if export_name is not None:
            normalized_required_exports.add(export_name)
    # Host-facing publication roots have one generated authority in
    # ``output_export_policy.essential_exports``.  Keeping a second literal
    # list here previously let linked-result decoders lose ``molt_len`` and
    # ``molt_index`` while a superficially similar subset remained exported.
    normalized_required_exports.update(_api("_ESSENTIAL_EXPORTS"))
    raw_dynamic_exports = os.environ.get(
        "MOLT_WASM_DYNAMIC_REQUIRED_EXPORTS", ""
    ).strip()
    if raw_dynamic_exports:
        normalized_required_exports.update(
            name.strip() for name in raw_dynamic_exports.split(",") if name.strip()
        )

    # The shared runtime is app-independent and retains the canonical public ABI.
    # Cargo/LLVM runtime generation owns body optimization. The linker owns only
    # export filtering and its verified structural post-link cleanup; the old
    # second Binaryen lane spent minutes reoptimizing the same full export graph
    # and could delete the public ABI.
    cache_started = time.perf_counter()
    metric_prefix = "runtime_tree_shake_cache"
    _api("_cache_metric_add")(operation_counts, f"{metric_prefix}_requests", 1)
    facts_authority_digest = _api("_wasm_facts_cache_authority_digest")(
        facts_provider,
        runtime_data,
    )
    cache_key = _api("_tree_shake_runtime_cache_key")(
        runtime_data=runtime_data,
        normalized_required_exports=normalized_required_exports,
        facts_authority_digest=facts_authority_digest,
    )
    cache_entry = _api("_wasm_link_cache_entry")(
        "runtime_tree_shake",
        _api("_TREE_SHAKE_RUNTIME_CACHE_SCHEMA"),
        cache_key,
        cache_root=_api("_wasm_link_cache_root")(),
    )
    with _api("_locked_wasm_link_cache_entry")(cache_entry) as lock_wait_ms:
        _api("_cache_metric_add")(
            operation_counts, f"{metric_prefix}_lock_wait_ms", lock_wait_ms
        )
        lookup_started = time.perf_counter()
        cached = _api("_read_wasm_link_cache_entry")(cache_entry)
        _api("_cache_metric_add")(
            operation_counts,
            f"{metric_prefix}_lookup_ms",
            (time.perf_counter() - lookup_started) * 1000.0,
        )
        if cached.data is not None:
            _api("_cache_metric_add")(operation_counts, f"{metric_prefix}_hits", 1)
            _api("_cache_metric_add")(
                operation_counts, f"{metric_prefix}_bytes_read", cached.bytes_read
            )
            _api("_cache_metric_add")(
                operation_counts,
                f"{metric_prefix}_wall_ms",
                (time.perf_counter() - cache_started) * 1000.0,
            )
            print(f"Runtime tree-shake cache hit: {cache_entry.root}", file=sys.stderr)
            return cached.data
        _api("_cache_metric_add")(operation_counts, f"{metric_prefix}_misses", 1)
        if cached.status == "corrupt":
            _api("_cache_metric_add")(
                operation_counts, f"{metric_prefix}_corruptions", 1
            )
            _api("_invalidate_wasm_link_cache_entry")(cache_entry)

    sections = _api("_parse_sections")(runtime_data)

    # Rewrite export section: keep memory/table/global exports and only
    # function exports that are in the required set.
    new_sections: list[tuple[int, bytes]] = []
    kept_exports = 0
    stripped_exports = 0

    for section_id, payload in sections:
        if section_id != 7:  # not export section
            new_sections.append((section_id, payload))
            continue

        # Parse and filter exports.
        offset = 0
        count, offset = _api("_read_varuint")(payload, offset)
        filtered: list[tuple[str, int, int]] = []  # (name, kind, index)
        for _ in range(count):
            name, offset = _api("_read_string")(payload, offset)
            if offset >= len(payload):
                raise ValueError("Unexpected EOF reading export kind")
            kind = payload[offset]
            offset += 1
            index, offset = _api("_read_varuint")(payload, offset)
            if kind != 0:
                # Memory (2), table (1), global (3) -- always keep.
                filtered.append((name, kind, index))
                kept_exports += 1
            elif name in normalized_required_exports:
                filtered.append((name, kind, index))
                kept_exports += 1
            else:
                stripped_exports += 1

        # Rebuild export section.
        new_payload = bytearray()
        new_payload.extend(_api("_write_varuint")(len(filtered)))
        for name, kind, index in filtered:
            new_payload.extend(_api("_write_string")(name))
            new_payload.append(kind)
            new_payload.extend(_api("_write_varuint")(index))
        new_sections.append((7, bytes(new_payload)))

    print(
        f"Runtime tree-shake: kept {kept_exports} exports, "
        f"stripped {stripped_exports} unused function exports",
        file=sys.stderr,
    )

    stripped_data = _api("_build_sections")(new_sections)
    optimized_baseline = _api("_post_link_optimize")(
        stripped_data,
        preserve_exports=normalized_required_exports,
        facts_provider=facts_provider,
    )
    if len(optimized_baseline) != len(stripped_data):
        print(
            f"Runtime post-link optimize: {len(stripped_data):,} -> {len(optimized_baseline):,} bytes "
            f"({len(stripped_data) - len(optimized_baseline):,} bytes eliminated)",
            file=sys.stderr,
        )

    with _api("_locked_wasm_link_cache_entry")(cache_entry) as lock_wait_ms:
        _api("_cache_metric_add")(
            operation_counts, f"{metric_prefix}_lock_wait_ms", lock_wait_ms
        )
        cached = _api("_read_wasm_link_cache_entry")(cache_entry)
        if cached.data is not None:
            _api("_cache_metric_add")(operation_counts, f"{metric_prefix}_hits", 1)
            _api("_cache_metric_add")(
                operation_counts, f"{metric_prefix}_bytes_read", cached.bytes_read
            )
            return cached.data
        _api("_publish_wasm_link_cache_result")(
            cache_entry,
            optimized_baseline,
            metrics=operation_counts,
            metric_prefix=metric_prefix,
            label="Runtime structural optimize",
            payload={
                "result_kind": "runtime-generation-plus-structural-link-cleanup",
                "kept_exports": kept_exports,
                "stripped_exports": stripped_exports,
            },
        )
    _api("_cache_metric_add")(
        operation_counts,
        f"{metric_prefix}_wall_ms",
        (time.perf_counter() - cache_started) * 1000.0,
    )
    return optimized_baseline


def _optimize_split_app_module(
    app_data: bytes,
    *,
    reference_data: bytes | None,
    optimize: bool,
    optimize_level: str,
    contract_keep_set: set[str],
    attestation: dict[str, object] | None = None,
    operation_counts: dict[str, int | float] | None = None,
    facts_provider: Callable[[bytes], dict[str, object]],
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
    _api("_cache_metric_add")(operation_counts, f"{metric_prefix}_requests", 1)
    facts_authority_digest = _api("_wasm_facts_cache_authority_digest")(
        facts_provider,
        app_data,
    )
    wasm_opt_identity = None
    if optimize:
        wasm_opt_path = _api("find_wasm_opt")()
        wasm_opt_identity = (
            _api("_wasm_opt_executable_identity")(wasm_opt_path)
            if wasm_opt_path is not None
            else None
        )
        if wasm_opt_identity is None:
            _api("_cache_metric_add")(
                operation_counts, f"{metric_prefix}_identity_errors", 1
            )
            _api("_cache_metric_add")(
                operation_counts,
                f"{metric_prefix}_wall_ms",
                (time.perf_counter() - cache_started) * 1000.0,
            )
            raise RuntimeError(
                "required split-app wasm optimization has no stable executable identity"
            )
    cache_key = _api("_split_app_optimize_cache_key")(
        app_data=app_data,
        reference_data=reference_data,
        optimize=optimize,
        optimize_level=optimize_level,
        contract_keep_set=contract_keep_set,
        facts_authority_digest=facts_authority_digest,
        wasm_opt_identity=wasm_opt_identity,
    )
    assert cache_key is not None
    cache_entry = _api("_wasm_link_cache_entry")(
        "split_app_optimize",
        _api("_SPLIT_APP_OPTIMIZE_CACHE_SCHEMA"),
        cache_key,
        cache_root=_api("_wasm_link_cache_root")(),
    )
    with _api("_locked_wasm_link_cache_entry")(cache_entry) as lock_wait_ms:
        _api("_cache_metric_add")(
            operation_counts, f"{metric_prefix}_lock_wait_ms", lock_wait_ms
        )
        lookup_started = time.perf_counter()
        cached = _api("_read_wasm_link_cache_entry")(cache_entry)
        _api("_cache_metric_add")(
            operation_counts,
            f"{metric_prefix}_lookup_ms",
            (time.perf_counter() - lookup_started) * 1000.0,
        )
        if cached.data is not None:
            _api("_cache_metric_add")(operation_counts, f"{metric_prefix}_hits", 1)
            _api("_cache_metric_add")(
                operation_counts, f"{metric_prefix}_bytes_read", cached.bytes_read
            )
            if attestation is not None:
                attestation.update(cached.payload or {})
                attestation["cache_hit"] = True
            _api("_cache_metric_add")(
                operation_counts,
                f"{metric_prefix}_wall_ms",
                (time.perf_counter() - cache_started) * 1000.0,
            )
            return cached.data
        _api("_cache_metric_add")(operation_counts, f"{metric_prefix}_misses", 1)
        if cached.status == "corrupt":
            _api("_cache_metric_add")(
                operation_counts, f"{metric_prefix}_corruptions", 1
            )
            _api("_invalidate_wasm_link_cache_entry")(cache_entry)

        optimized = _api("_post_link_optimize")(
            app_data,
            reference_data=reference_data,
            preserve_exports=contract_keep_set,
            preserve_reference_exports=False,
            facts_provider=facts_provider,
        )
        stripped = _api("_strip_unused_module_function_imports")(
            optimized,
            module_name="molt_runtime",
            facts=facts_provider(optimized),
        )
        if stripped is not None:
            optimized = stripped
        result = optimized
        active_attestation = attestation if attestation is not None else {}
        if optimize:
            assert wasm_opt_identity is not None
            optimizer_policy = _api("wasm_link_policy")(optimize_level)
            with tempfile.TemporaryDirectory(prefix="molt-split-app-opt-") as tmp:
                app_path = Path(tmp) / "app_split_preopt.wasm"
                app_path.write_bytes(optimized)
                required_function_exports = (
                    set(_api("_collect_function_exports")(optimized))
                    & contract_keep_set
                )
                _api("_cache_metric_add")(
                    operation_counts, "split_app_wasm_opt_runs", 1
                )
                optimizer_ok = _api("_run_wasm_opt_via_optimize")(
                    app_path,
                    level=optimizer_policy.level,
                    converge=optimizer_policy.converge,
                    required_exports=required_function_exports,
                    apply_level=optimizer_policy.apply_level,
                    extra_passes=optimizer_policy.extra_passes,
                    attestation=active_attestation,
                )
                _api("_record_wasm_opt_attestation_cache_metrics")(
                    operation_counts, metric_prefix, active_attestation
                )
                if optimizer_ok:
                    result = app_path.read_bytes()
                else:
                    failure = str(
                        active_attestation.get("error", "unknown optimizer failure")
                    )
                    raise RuntimeError(
                        f"required split-app wasm optimization failed: {failure}"
                    )
                if (
                    active_attestation.get("wasm_opt_path") != wasm_opt_identity[0]
                    or active_attestation.get("wasm_opt_sha256") != wasm_opt_identity[1]
                ):
                    raise RuntimeError(
                        "required split-app wasm optimization crossed executable identity"
                    )
        cache_payload = dict(active_attestation)
        cache_payload["cache_hit"] = False
        if optimize:
            assert wasm_opt_identity is not None
            cache_payload.update(
                {
                    "wasm_opt_path": wasm_opt_identity[0],
                    "wasm_opt_sha256": wasm_opt_identity[1],
                    "wasm_opt_version": wasm_opt_identity[2],
                }
            )
        _api("_publish_wasm_link_cache_result")(
            cache_entry,
            result,
            metrics=operation_counts,
            metric_prefix=metric_prefix,
            label="Split app optimize",
            payload=cache_payload,
        )
        _api("_cache_metric_add")(
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
    required_exports: set[str] | None = None,
    apply_level: bool | None = None,
    extra_passes: Sequence[str] | None = None,
    attestation: dict[str, object] | None = None,
) -> bool:
    """Run the canonical atomic optimizer and record its attestation."""

    policy = _api("wasm_link_policy")(level)
    resolved_converge = policy.converge if converge is None else converge
    resolved_apply_level = policy.apply_level if apply_level is None else apply_level
    resolved_extra_passes = (
        list(extra_passes) if extra_passes is not None else list(policy.extra_passes)
    )

    pre_size = linked.stat().st_size
    if required_exports is None:
        try:
            required_exports = set(
                _api("_collect_function_exports")(linked.read_bytes())
            )
        except (OSError, ValueError):
            required_exports = set()
    result = _api("optimize_wasm")(
        linked,
        output_path=linked,
        level=level,
        extra_passes=resolved_extra_passes,
        converge=resolved_converge,
        required_exports=required_exports,
        apply_level=resolved_apply_level,
    )

    if not result["ok"]:
        err = result.get("error", "unknown error")
        if attestation is not None:
            attestation.update(
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

    if attestation is not None:
        attestation.update(
            {
                "ok": True,
                "status": result.get("status", "success"),
                "binaryen_version": result.get("binaryen_version", ""),
                "wasm_opt_path": result.get("wasm_opt_path"),
                "wasm_opt_sha256": result.get("wasm_opt_sha256"),
                "pipeline": result.get("pipeline", []),
                "before": result.get("before", {}),
                "after": result.get("after", {}),
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
