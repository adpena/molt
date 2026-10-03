"""Shared backend environment contract, also embedded by molt-ir.

Only backend_environment.json authors membership. Keep absent and empty values
distinct: several pass controls intentionally test presence, not truthiness.
"""
from __future__ import annotations

import json
import os
from pathlib import Path
from typing import Mapping

_CATALOG = json.loads(Path(__file__).with_suffix(".json").read_text(encoding="utf-8"))
_GROUPS = {"common", "native", "wasm", "diagnostic", "observation", "resource", "transport", "runtime_identity", "build_identity"}
if not isinstance(_CATALOG, dict) or _CATALOG.get("schema") != 1 or set(_CATALOG) != _GROUPS | {"schema"}:
    raise ValueError("unsupported backend environment catalog")
_CATALOG.pop("schema")
if any(not isinstance(names, list) or not all(isinstance(name, str) and name and name.isascii() and name.replace("_", "").isalnum() and name == name.upper() for name in names) for names in _CATALOG.values()):
    raise ValueError("malformed backend environment catalog group")
_ALL = [name for names in _CATALOG.values() for name in names]
if len(_ALL) != len(set(_ALL)):
    raise ValueError("duplicate backend environment catalog entry")


def environment_keys(*groups: str) -> tuple[str, ...]:
    return tuple(name for group in groups for name in _CATALOG[group])


def compilation_diagnostics_requested(env: Mapping[str, str] | None = None) -> bool:
    """Requested compilation diagnostics require execution, including empty values."""
    if env is None:
        env = os.environ
    return any(name in env for name in environment_keys("diagnostic"))


def codegen_environment_inputs(*, is_wasm: bool, env: Mapping[str, str]) -> dict[str, str]:
    keys = environment_keys("common", "native", "runtime_identity", "build_identity", "diagnostic", "observation")
    if is_wasm:
        keys += environment_keys("wasm")
    return {name: env[name] for name in sorted(keys) if name in env}
