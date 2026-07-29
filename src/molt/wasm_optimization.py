from __future__ import annotations

from collections.abc import Sequence
from dataclasses import dataclass

WASM_OPT_LEVELS = ("O1", "O2", "O3", "O4", "Os", "Oz")
WASM_OPT_DEV_DEFAULT = "O1"
WASM_OPT_RELEASE_DEFAULT = "Oz"

# Explicit feature set instead of --all-features. Binaryen's
# --enable-custom-descriptors and GC rec-group encodings are not accepted by
# every Molt runtime target yet, so all optimizer consumers share this exact
# cross-engine contract.
WASM_OPT_FEATURE_FLAGS = (
    "--enable-bulk-memory",
    "--enable-mutable-globals",
    "--enable-sign-ext",
    "--enable-nontrapping-float-to-int",
    "--enable-simd",
    "--enable-multivalue",
    "--enable-reference-types",
    "--disable-gc",
    "--enable-tail-call",
    "--disable-custom-descriptors",
)

_WASM_OPT_OZ_PASSES = (
    "--remove-unused-module-elements",
    "--strip-debug",
    "--strip-producers",
    "--dae-optimizing",
    "--simplify-locals",
    "--merge-blocks",
    "--dce",
    "--vacuum",
    "--zero-filled-memory",
    "--memory-packing",
)
_WASM_OPT_O3_PASSES = (
    "--closed-world",
    "--remove-unused-module-elements",
    "--remove-unused-names",
    "--strip-producers",
    "--coalesce-locals",
    "--reorder-locals",
    "--merge-locals",
    "--dce",
    "--vacuum",
    "--inlining",
    "--flatten",
    "--local-cse",
    "--optimize-stack-ir",
    "--reorder-functions",
    "--precompute",
)


@dataclass(frozen=True, slots=True)
class WasmOptPolicy:
    level: str
    apply_level: bool
    converge: bool
    extra_passes: tuple[str, ...]

    @property
    def pipeline(self) -> tuple[str, ...]:
        return wasm_opt_pipeline(
            self.level,
            extra_passes=self.extra_passes,
            converge=self.converge,
            apply_level=self.apply_level,
        )


def wasm_opt_converges(level: str) -> bool:
    """Return whether the selected profile pays for fixed-point optimization."""

    if level not in WASM_OPT_LEVELS:
        raise ValueError(f"unsupported wasm-opt level: {level!r}")
    return level != WASM_OPT_DEV_DEFAULT


def wasm_opt_pipeline(
    level: str,
    *,
    extra_passes: Sequence[str] = (),
    converge: bool | None = None,
    apply_level: bool = True,
) -> tuple[str, ...]:
    """Build the canonical Binaryen argument pipeline for every consumer."""

    if level not in WASM_OPT_LEVELS:
        raise ValueError(f"unsupported wasm-opt level: {level!r}")
    resolved_converge = wasm_opt_converges(level) if converge is None else converge
    pipeline: list[str] = []
    if apply_level:
        pipeline.append(f"-{level}")
    pipeline.extend(WASM_OPT_FEATURE_FLAGS)
    pipeline.append("--strip-producers")
    if resolved_converge:
        pipeline.append("--converge")
    pipeline.extend(extra_passes)
    return tuple(dict.fromkeys(pipeline))


def wasm_link_policy(level: str) -> WasmOptPolicy:
    """Return the canonical post-link optimizer policy for an artifact."""

    level_passes = {
        "Oz": _WASM_OPT_OZ_PASSES,
        "O3": _WASM_OPT_O3_PASSES,
    }.get(level, ())
    return WasmOptPolicy(
        level=level,
        apply_level=True,
        converge=False,
        extra_passes=level_passes,
    )
