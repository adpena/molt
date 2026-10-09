from __future__ import annotations

from contextlib import ExitStack
from dataclasses import dataclass
from pathlib import Path
from types import MappingProxyType
from typing import Mapping

from molt.cli import wasm_link_inputs
from molt.wasi_sdk_identity import WasiCAbiProjection
from molt.cli.native_symbol_inspection import (
    _native_symbol_facts_admission,
)


WASM_LIBC_LINK_IMPORT_CLASS = "wasm_libc_link_import"
WASM_COMPILER_RT_LINK_IMPORT_CLASS = "wasm_compiler_rt_link_import"
WASM_LIBCXX_LINK_IMPORT_CLASS = "wasm_libcxx_link_import"

# Order is link policy: use the smallest baseline provider that really defines
# a symbol.  Later provider families are staged only when the earlier families
# do not own it.  The order is stable and therefore deterministic when provider
# archives expose an intentional weak/compatibility overlap.
_PROVIDER_CLASS_PRECEDENCE = (
    WASM_LIBC_LINK_IMPORT_CLASS,
    WASM_COMPILER_RT_LINK_IMPORT_CLASS,
    WASM_LIBCXX_LINK_IMPORT_CLASS,
)


@dataclass(frozen=True)
class ExternalLinkProviderSurface:
    primitive_class: str
    archives: tuple[Path, ...]
    symbols: frozenset[str]


def _resolved_provider_archives(
    target_triple: str,
    primitive_classes: frozenset[str] | None,
    plan: WasiCAbiProjection | None,
) -> tuple[tuple[str, tuple[Path, ...]], ...]:
    requested = tuple(
        name
        for name in _PROVIDER_CLASS_PRECEDENCE
        if primitive_classes is None or name in primitive_classes
    )
    if target_triple == "wasm32-unknown-unknown" or not requested:
        return tuple((name, ()) for name in requested)
    if target_triple != "wasm32-wasip1":
        raise ValueError(f"unsupported WASM provider target: {target_triple}")
    plan = plan or wasm_link_inputs.resolve_wasi_c_abi_plan()
    archives = []
    for name in requested:
        if name == WASM_LIBC_LINK_IMPORT_CLASS:
            paths = (plan.path("long_double"), plan.path("libc"))
        elif name == WASM_COMPILER_RT_LINK_IMPORT_CLASS:
            paths = (plan.path("compiler_rt"),)
        else:
            paths = wasm_link_inputs.wasm_cxx_runtime_archives(plan=plan)
        archives.append((name, paths))
    return tuple(archives)


def wasm_external_link_provider_surfaces(
    target_triple: str = "wasm32-wasip1",
    *,
    primitive_classes: frozenset[str] | None = None,
    plan: WasiCAbiProjection | None = None,
    archive_paths: Mapping[Path, Path] | None = None,
) -> tuple[ExternalLinkProviderSurface, ...]:
    """Project current provider facts under one owned admission per archive.

    The native symbol authority owns content caching. Provider surfaces, sets
    and precedence maps are cheap projections, never a second detached cache.
    Keep the complete provider set owned until its last projection is built.
    """
    surfaces: list[ExternalLinkProviderSurface] = []
    with ExitStack() as owned:
        for primitive_class, archives in _resolved_provider_archives(
            target_triple, primitive_classes, plan
        ):
            if archive_paths is not None:
                archives = tuple(archive_paths[path] for path in archives)
            symbols: set[str] = set()
            for archive in archives:
                _opened, _identity, facts = owned.enter_context(
                    _native_symbol_facts_admission(
                        archive,
                        archive=True,
                        target_triple=target_triple.strip().lower(),
                    )
                )
                symbols.update(facts.defined)
            surfaces.append(
                ExternalLinkProviderSurface(
                    primitive_class=primitive_class,
                    archives=archives,
                    symbols=frozenset(symbols),
                )
            )
    return tuple(surfaces)


def wasm_external_link_provider_symbol_classes(
    target_triple: str = "wasm32-wasip1",
) -> Mapping[str, str]:
    """Map provider exports to their canonical link-policy precedence."""
    classes: dict[str, str] = {}
    surfaces = {
        surface.primitive_class: surface
        for surface in wasm_external_link_provider_surfaces(target_triple)
    }
    for primitive_class in _PROVIDER_CLASS_PRECEDENCE:
        for symbol in surfaces[primitive_class].symbols:
            classes.setdefault(symbol, primitive_class)
    return MappingProxyType(classes)


def wasm_external_link_provider_symbols(
    *,
    primitive_classes: frozenset[str] | None = None,
    target_triple: str = "wasm32-wasip1",
    plan: WasiCAbiProjection | None = None,
    archive_paths: Mapping[Path, Path] | None = None,
) -> frozenset[str]:
    return frozenset(
        symbol
        for surface in wasm_external_link_provider_surfaces(
            target_triple,
            primitive_classes=primitive_classes,
            plan=plan,
            archive_paths=archive_paths,
        )
        for symbol in surface.symbols
    )
