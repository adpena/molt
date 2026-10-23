from __future__ import annotations

from collections.abc import Sequence
from dataclasses import dataclass

WASM_OPT_LEVELS = ("O1", "O2", "O3", "O4", "Os", "Oz")
# Dev builds run no post-link optimizer, as Go and Cargo debug builds run none.
# For a 91-line asyncio program, `-O1 -g` over the 75 MB linked module cost 267
# CPU seconds, 54 s and 4.2 GB peak RSS to save 10.4 MB; Node then ran the
# smaller module in 2.1-2.3 s against 4.0-4.6 s (2026-10-23, WASM optimization
# plan section 4.4). An explicit `--wasm-opt-level` selects a level for dev.
WASM_OPT_DEV_DEFAULT: str | None = None
WASM_OPT_RELEASE_DEFAULT = "Oz"
# The one level that runs a single pass rather than to a fixed point.
_SINGLE_PASS_LEVEL = "O1"

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
    "--enable-exception-handling",
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
    preserve_debug: bool = False

    @property
    def pipeline(self) -> tuple[str, ...]:
        return wasm_opt_pipeline(
            self.level,
            extra_passes=self.extra_passes,
            converge=self.converge,
            apply_level=self.apply_level,
            preserve_debug=self.preserve_debug,
        )


def wasm_opt_converges(level: str) -> bool:
    """Return whether the selected profile pays for fixed-point optimization."""

    if level not in WASM_OPT_LEVELS:
        raise ValueError(f"unsupported wasm-opt level: {level!r}")
    return level != _SINGLE_PASS_LEVEL


def wasm_opt_pipeline(
    level: str,
    *,
    extra_passes: Sequence[str] = (),
    converge: bool | None = None,
    apply_level: bool = True,
    preserve_debug: bool = False,
) -> tuple[str, ...]:
    """Build the canonical Binaryen argument pipeline for every consumer."""

    if level not in WASM_OPT_LEVELS:
        raise ValueError(f"unsupported wasm-opt level: {level!r}")
    if preserve_debug and {"--strip-debug", "--remove-unused-names"} & set(
        extra_passes
    ):
        raise ValueError("debug-preserving wasm optimization cannot strip debug names")
    resolved_converge = wasm_opt_converges(level) if converge is None else converge
    pipeline: list[str] = []
    if apply_level:
        pipeline.append(f"-{level}")
    pipeline.extend(WASM_OPT_FEATURE_FLAGS)
    pipeline.append("--strip-producers")
    if preserve_debug:
        # Binaryen emits the name section only when debug output is requested.
        pipeline.append("-g")
    if resolved_converge:
        pipeline.append("--converge")
    pipeline.extend(extra_passes)
    return tuple(dict.fromkeys(pipeline))


def wasm_link_policy(level: str, *, preserve_debug: bool = False) -> WasmOptPolicy:
    """Return the canonical post-link optimizer policy for an artifact."""

    level_passes = {
        "Oz": _WASM_OPT_OZ_PASSES,
        "O3": _WASM_OPT_O3_PASSES,
    }.get(level, ())
    if preserve_debug:
        level_passes = tuple(
            flag
            for flag in level_passes
            if flag not in {"--strip-debug", "--remove-unused-names"}
        )
    return WasmOptPolicy(
        level=level,
        apply_level=True,
        converge=False,
        extra_passes=level_passes,
        preserve_debug=preserve_debug,
    )
