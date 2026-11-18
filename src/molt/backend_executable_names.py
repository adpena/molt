"""Backend variants and executable names for publication and discovery.

Names identify discovery candidates, never artifact provenance or process
ownership. Admission still requires the content-bound build receipt.
"""

from __future__ import annotations

from collections.abc import Sequence
from typing import Literal, get_args

from molt.host_tool_products import HostToolProduct


CodegenBackend = Literal["cranelift", "llvm"]
CODEGEN_BACKENDS: tuple[CodegenBackend, ...] = get_args(CodegenBackend)


def default_codegen_backend(*, wasm: bool, release: bool) -> CodegenBackend:
    """The backend `--backend auto` selects: one rule for every consumer.

    LLVM for WebAssembly (its wasm32 target is the only WebAssembly code
    generator) and for a release build: only the LLVM backend consumes final
    TIR, so only it applies the late-TIR proofs (about 3x faster programs on
    the large benchmarks, 2026-11-18). Cranelift for a dev build, which it
    compiles about 3.3x faster. Owner decision 2026-11-18; Cranelift takes
    final TIR at codegen-walker W3.
    """
    return "llvm" if wasm or release else "cranelift"


DEFAULT_BACKEND_FEATURES: tuple[str, ...] = ("native-backend",)


def backend_features_for_target(
    *,
    is_wasm: bool,
    codegen_backend: CodegenBackend,
) -> tuple[str, ...]:
    """Resolve the same feature identity for builds, names and cache keys.

    ``codegen_backend`` is the resolved selection (``--backend``; ``auto``
    follows ``default_codegen_backend``). The process environment never
    selects it. A wasm32 program compiles through LLVM's wasm32 target, a
    native job of the same backend binary.
    """
    if codegen_backend not in CODEGEN_BACKENDS:
        raise ValueError(f"unknown codegen backend: {codegen_backend!r}")
    if is_wasm and codegen_backend != "llvm":
        raise ValueError("a wasm32 target compiles through the llvm backend")
    if codegen_backend == "llvm":
        return (*DEFAULT_BACKEND_FEATURES, "llvm")
    return DEFAULT_BACKEND_FEATURES


def backend_executable_name(
    *, os_name: str, features: Sequence[str] | None = None
) -> str:
    """Name the Cargo output (None), or an independently published variant."""
    return HostToolProduct.BACKEND.executable_name(os_name=os_name, features=features)


_BACKEND_IMAGES = frozenset(
    backend_executable_name(os_name=os_name, features=features)
    for os_name in ("nt", "posix")
    for features in (
        None,
        (),
        DEFAULT_BACKEND_FEATURES,
        (*DEFAULT_BACKEND_FEATURES, "llvm"),
    )
)


def is_backend_executable_name(name: str) -> bool:
    """Recognize a backend image basename for sampling and quiescence only."""
    return name.strip().casefold() in _BACKEND_IMAGES


_HOST_TOOL_IMAGES = _BACKEND_IMAGES | frozenset(
    product.executable_name(os_name=os_name, features=features)
    for product in HostToolProduct
    if product is not HostToolProduct.BACKEND
    for os_name in ("nt", "posix")
    for features in (None, product.required_features)
)


def is_host_tool_executable_name(name: str) -> bool:
    """Recognize host-tool candidates for sampling, never process ownership."""
    return name.strip().casefold() in _HOST_TOOL_IMAGES
