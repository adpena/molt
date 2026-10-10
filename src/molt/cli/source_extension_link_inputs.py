"""Resolve source-extension link inputs from captured or selected tools."""

from __future__ import annotations

from collections.abc import Mapping
import os

from molt import source_extension_link_inputs
from molt.exact_json import loads_exact
from molt.wasi_sdk_identity import WasiCAbiProjection


def resolve_source_extension_link_inputs(
    target_triple: str,
    *,
    wasi_c_abi: WasiCAbiProjection | None,
    environment: Mapping[str, str] | None = None,
) -> source_extension_link_inputs.SourceExtensionLinkInputs:
    environment = os.environ if environment is None else environment
    env_name = source_extension_link_inputs.SOURCE_EXTENSION_LINK_INPUTS_ENV
    if env_name in environment:
        try:
            payload = loads_exact(environment[env_name])
        except (TypeError, ValueError) as exc:
            raise ValueError(
                "invalid captured source-extension link-input environment"
            ) from exc
        captured = source_extension_link_inputs.validate_source_extension_link_inputs(
            payload, target_triple=target_triple
        )
        captured.verify_c_abi(wasi_c_abi)
        return captured
    if target_triple != "wasm32-wasip1":
        if wasi_c_abi is not None:
            raise ValueError("non-WASI source extension has an unexpected C ABI plan")
        return source_extension_link_inputs.project_source_extension_link_inputs(
            target_triple, None
        )
    if wasi_c_abi is None:
        raise ValueError("WASI source extension requires its selected C ABI plan")
    captured = source_extension_link_inputs.project_source_extension_link_inputs(
        target_triple, wasi_c_abi
    )
    captured.verify_c_abi(wasi_c_abi)
    return captured
