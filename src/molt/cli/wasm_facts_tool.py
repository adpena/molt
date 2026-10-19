"""Admit the standalone host WASM facts product through compiler build custody."""

from __future__ import annotations

import os
from collections.abc import Mapping
from pathlib import Path

from molt.cli.backend_binary import _ensure_backend_binary
from molt.cli.command_runtime import _resolve_timeout_env
from molt.cli.runtime_paths import _cargo_profile_dir
from molt.cli.rust_cpu_policy import RustCpuPolicy
from molt.compiler_distribution import installed_compiler
from molt.dx import project_cargo_target_dir
from molt.host_tool_products import HostToolProduct

SOURCE_CARGO_PROFILE = "release-fast"


def ensure_wasm_facts_tool(root: Path, *, env: Mapping[str, str]) -> Path:
    """Return one admitted immutable facts executable, never a PATH candidate.

    Installed execution requires its manifest-owned prebuilt tool. Source
    execution shares the existing Cargo plan, source and observed-input
    receipts, generation store and capability probe with the backend compiler.
    Source inspection uses the optimized iteration profile independently of
    compiler development profiles; installed tools retain their release profile.
    Both host products use the baseline CPU policy for WASM builds.
    """
    product = HostToolProduct.WASM_FACTS
    features = product.required_features
    installed = installed_compiler(root)
    if installed is not None:
        cargo_profile = installed.wasm_facts["profile"]
        coordinate = installed.wasm_facts_binary
    else:
        cargo_profile = SOURCE_CARGO_PROFILE
        name = product.executable_name(os_name=os.name, features=features)
        coordinate = (
            project_cargo_target_dir(root, env)
            / _cargo_profile_dir(cargo_profile)
            / name
        )
    timeout, error = _resolve_timeout_env("MOLT_CARGO_TIMEOUT", environment=env)
    if error:
        raise ValueError(error)
    result = _ensure_backend_binary(
        coordinate,
        cargo_timeout=timeout,
        json_output=True,
        cargo_profile=cargo_profile,
        project_root=root,
        backend_features=features,
        rust_cpu=RustCpuPolicy.BASELINE,
        product=product,
        environment=env,
    )
    if not result:
        raise ValueError(f"WASM facts tool admission failed: {result.message}")
    assert result.binary is not None
    return result.binary
