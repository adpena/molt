from __future__ import annotations

import functools
from collections.abc import Mapping
from pathlib import Path

from molt.cli.config_resolution import (
    AUTO_STDLIB_PROFILE,
    DEFAULT_STDLIB_PROFILE,
)
from molt.cli import module_resolution as _module_resolution
from molt.source_root import compiler_source_root


@functools.lru_cache(maxsize=8)
def _stdlib_allowlist_cached(source_text: str) -> frozenset[str]:
    allowlist: set[str] = set()
    for line in source_text.splitlines():
        if not line.startswith("|"):
            continue
        if line.startswith("| ---"):
            continue
        parts = [part.strip() for part in line.strip().strip("|").split("|")]
        if not parts:
            continue
        module_name = parts[0]
        if not module_name or module_name == "Module":
            continue
        for entry in module_name.split("/"):
            entry = entry.strip()
            if entry:
                allowlist.add(entry)
    return frozenset(allowlist)


def _stdlib_allowlist() -> set[str]:
    spec_path = (
        compiler_source_root()
        / "docs/spec/areas/compat/surfaces/stdlib/stdlib_surface_matrix.md"
    )
    if not spec_path.exists():
        return set()
    # Only parsing is cached. Root selection and policy bytes stay live,
    # including same-size edits with restored modification timestamps.
    return set(_stdlib_allowlist_cached(spec_path.read_text(encoding="utf-8")))


_CORE_STDLIB_MODULES_FULL = (
    "builtins",
    "sys",
    "types",
    "importlib",
    "importlib.util",
    "importlib.machinery",
)


_CORE_STDLIB_MODULES_MICRO = (
    "builtins",
    "sys",
)


def _core_stdlib_module_names_for_profile(
    stdlib_profile: str | None,
) -> tuple[str, ...]:
    from molt.frontend._types import BUILTIN_FUNC_SPECS

    profile = stdlib_profile or DEFAULT_STDLIB_PROFILE
    if profile in {AUTO_STDLIB_PROFILE, "micro", "edge", "standard", "server"}:
        core = _CORE_STDLIB_MODULES_MICRO
    else:
        core = _CORE_STDLIB_MODULES_FULL
    # Native public callables retain their real provider module. Include those
    # initializers in the module inventory without emitting an eager import.
    # Runtime publication initializes a provider only when the target resolver
    # admits its callable; e.g. a Pure WASM guest must not initialize file I/O.
    providers = sorted(
        {spec.module for spec in BUILTIN_FUNC_SPECS.values()} - set(core)
    )
    return (*core, *providers)


def _ensure_core_stdlib_modules(
    module_graph: dict[str, Path],
    stdlib_root: Path,
    stdlib_profile: str = DEFAULT_STDLIB_PROFILE,
) -> None:
    """Add the profile's unconditional core stdlib modules to the graph.

    ``stdlib_profile`` is the value `build()` resolved through the single
    config authority (`config_resolution.resolve_stdlib_profile`) and also
    passes to the staticlib selector, so the closure and the linked staticlib
    cannot disagree. The process environment is not consulted here.
    """
    core_modules = _core_stdlib_module_names_for_profile(stdlib_profile)
    for name in core_modules:
        path = _module_resolution._resolve_module_path(name, [stdlib_root])
        if path is not None:
            module_graph.setdefault(name, path)


def _looks_like_stdlib_module_name(module_name: str) -> bool:
    if module_name == "molt.stdlib" or module_name.startswith("molt.stdlib."):
        return True
    root = module_name.split(".", 1)[0]
    return root in {
        "__future__",
        "_collections_abc",
        "abc",
        "builtins",
        "collections",
        "dataclasses",
        "importlib",
        "os",
        "pathlib",
        "runpy",
        "signal",
        "sys",
        "test",
        "typing",
        "warnings",
        "zipfile",
        "zipimport",
    }


def _build_stdlib_like_module_flags(
    module_graph: Mapping[str, Path],
) -> dict[str, bool]:
    return {
        module_name: (
            _module_resolution._is_runtime_owned_module_path(module_path)
            or _looks_like_stdlib_module_name(module_name)
        )
        for module_name, module_path in sorted(module_graph.items())
    }
