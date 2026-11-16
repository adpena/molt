"""Runtime tree-shake and split-app optimization policy authority."""

from __future__ import annotations

from collections.abc import Mapping, Sequence
from pathlib import Path
import sys

from molt.wasm_optimization import wasm_link_policy
from molt.wasm_optimizer_identity import (
    WasmOptimizerExecutableIdentity,
)
from tools.wasm_optimize import optimize as optimize_wasm

TOOLS_ROOT = Path(__file__).resolve().parent


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
