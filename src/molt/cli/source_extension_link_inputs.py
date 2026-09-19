"""Resolve source-extension link inputs from captured or selected tools."""

from __future__ import annotations

from collections.abc import Mapping
import os

from molt import source_extension_link_inputs
from molt.cli import wasm_link_inputs
from molt.exact_json import loads_exact


def resolve_source_extension_link_inputs(
    target_triple: str,
    *,
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
        return source_extension_link_inputs.validate_source_extension_link_inputs(
            payload, target_triple=target_triple
        )
    if target_triple != "wasm32-wasip1":
        return source_extension_link_inputs.capture_source_extension_link_inputs(
            target_triple, None
        )
    path = wasm_link_inputs.wasm_compiler_builtins_archive(
        target_triple, environment=environment
    )
    if path is None:
        raise ValueError(
            "WASI source-extension target requires the selected Rust compiler-builtins archive for Meson configure links"
        )
    path = path.resolve(strict=True)
    return source_extension_link_inputs.capture_source_extension_link_inputs(
        target_triple, path
    )
