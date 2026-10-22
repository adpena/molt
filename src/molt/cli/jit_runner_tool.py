"""Admit the development JIT runner (``molt-jit``) through compiler build custody.

The runner embeds the native backend's code generator, so it builds with the
backend's Cargo profile and CPU policy and shares the backend's Cargo plan,
source receipts, product store and capability probe. Its probe compiles and
runs one function in executable memory, so a host whose W^X policy refuses JIT
code fails admission before any program runs.
"""

from __future__ import annotations

import os
from pathlib import Path

from molt.cli.backend_binary import _ensure_backend_binary
from molt.cli.backend_generation import BACKEND_GENERATIONS_DIR
from molt.cli.product_store import ProductStore
from molt.cli.runtime_paths import _cargo_profile_dir, _cargo_target_root
from molt.cli.rust_cpu_policy import RustCpuPolicy
from molt.host_tool_products import HostToolProduct


def jit_runner_coordinate(root: Path, *, cargo_profile: str) -> Path:
    product = HostToolProduct.JIT
    return (
        _cargo_target_root(root)
        / _cargo_profile_dir(cargo_profile)
        / product.executable_name(os_name=os.name, features=product.required_features)
    )


def ensure_jit_runner(
    root: Path,
    *,
    cargo_profile: str,
    rust_cpu: RustCpuPolicy,
    cargo_timeout: float | None,
    json_output: bool,
) -> Path:
    """Return one admitted immutable runner executable.

    Build request resolution already refused an installed compiler, which
    ships no runner (``run_tier.jit_tier_refusal``).
    """
    product = HostToolProduct.JIT
    result = _ensure_backend_binary(
        jit_runner_coordinate(root, cargo_profile=cargo_profile),
        cargo_timeout=cargo_timeout,
        json_output=json_output,
        cargo_profile=cargo_profile,
        project_root=root,
        backend_features=product.required_features,
        rust_cpu=rust_cpu,
        product=product,
    )
    if not result:
        raise ValueError(f"JIT runner admission failed: {result.message}")
    assert result.binary is not None
    return result.binary


def record_jit_runner_use(runner: Path) -> None:
    """Renew the product store entry of the runner a cached launch runs.

    ``runner`` is a generation of a stored host tool,
    ``<store>/<identity>/<variant>/.molt-backend-generations/<name>/<sha256>/<name>``;
    the store entry is the unit of retention (HF-390). An evicted entry
    raises ``GenerationEvicted``, a miss.
    """
    generations = runner.parent.parent
    if (
        generations.parent.name != BACKEND_GENERATIONS_DIR
        or generations.name != runner.name
    ):
        raise ValueError(f"not a JIT runner generation: {runner}")
    entry = generations.parent.parent.parent
    ProductStore(entry.parent).record_use(entry.name)
