from __future__ import annotations

import functools
from dataclasses import dataclass
from pathlib import Path
from types import MappingProxyType
from typing import Mapping

from molt.cli import wasm_link_inputs
from molt.cli.native_symbol_inspection import (
    _native_archive_global_symbol_facts,
    _native_symbol_artifact_identity,
    _require_unchanged_symbol_artifact,
)
from molt.toolchain_identity import StableRegularFileIdentity


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

_ProviderArchiveKey = tuple[str, tuple[StableRegularFileIdentity, ...]]
_ProviderResolutionKey = tuple[str, tuple[_ProviderArchiveKey, ...]]


@dataclass(frozen=True)
class ExternalLinkProviderSurface:
    primitive_class: str
    archives: tuple[Path, ...]
    symbols: frozenset[str]


def _resolved_provider_archives(
    target_triple: str,
) -> tuple[tuple[str, tuple[Path, ...]], ...]:
    if target_triple == "wasm32-wasip1":
        libc = wasm_link_inputs.wasm_wasi_libc_archive()
        compiler_rt = wasm_link_inputs.wasm_compiler_builtins_archive()
        libcxx = wasm_link_inputs.wasm_cxx_runtime_archives()
    else:
        libc = wasm_link_inputs.wasm_wasi_libc_archive(target_triple)
        compiler_rt = wasm_link_inputs.wasm_compiler_builtins_archive(target_triple)
        libcxx = wasm_link_inputs.wasm_cxx_runtime_archives(target_triple)
    return (
        (
            WASM_LIBC_LINK_IMPORT_CLASS,
            () if libc is None else (libc.resolve(strict=False),),
        ),
        (
            WASM_COMPILER_RT_LINK_IMPORT_CLASS,
            () if compiler_rt is None else (compiler_rt.resolve(strict=False),),
        ),
        (
            WASM_LIBCXX_LINK_IMPORT_CLASS,
            ()
            if libcxx is None
            else tuple(path.resolve(strict=False) for path in libcxx),
        ),
    )


def _provider_resolution_key(
    target_triple: str,
) -> _ProviderResolutionKey:
    key: list[_ProviderArchiveKey] = []
    for primitive_class, archives in _resolved_provider_archives(target_triple):
        archive_keys: list[StableRegularFileIdentity] = []
        for archive in archives:
            archive_keys.append(_native_symbol_artifact_identity(archive))
        key.append((primitive_class, tuple(archive_keys)))
    return target_triple.strip().lower(), tuple(key)


@functools.lru_cache(maxsize=8)
def _provider_surfaces_from_key(
    key: _ProviderResolutionKey,
) -> tuple[ExternalLinkProviderSurface, ...]:
    target_triple, provider_archives = key
    surfaces: list[ExternalLinkProviderSurface] = []
    for primitive_class, archive_keys in provider_archives:
        archives = tuple(Path(identity.path) for identity in archive_keys)
        symbols: set[str] = set()
        readable = bool(archives)
        for archive, identity in zip(archives, archive_keys, strict=True):
            facts = _native_archive_global_symbol_facts(
                archive,
                target_triple=target_triple,
                identity=identity,
            )
            symbols.update(facts.defined)
        surfaces.append(
            ExternalLinkProviderSurface(
                primitive_class=primitive_class,
                archives=archives,
                symbols=frozenset(symbols if readable else ()),
            )
        )
    return tuple(surfaces)


def _verify_provider_resolution_key(key: _ProviderResolutionKey) -> None:
    # Outer LRU hits also remain bound through their last returned projection.
    for _primitive_class, identities in key[1]:
        for identity in identities:
            _require_unchanged_symbol_artifact(identity.path, identity)


def wasm_external_link_provider_surfaces(
    target_triple: str = "wasm32-wasip1",
) -> tuple[ExternalLinkProviderSurface, ...]:
    """Return exact symbols owned by the archives the final linker will stage.

    This is the canonical external-native libc/compiler-rt/libc++ authority.
    It reads the resolved archive symbol tables directly, so upgrading a
    toolchain cannot silently retain a stale hand-maintained subset.  Missing or
    absent provider families expose an empty surface. An installed but unreadable
    provider raises a typed symbol-inspection error with attempted-tool evidence;
    it cannot publish or cache a partial provider surface.
    """

    key = _provider_resolution_key(target_triple)
    surfaces = _provider_surfaces_from_key(key)
    _verify_provider_resolution_key(key)
    return surfaces


def wasm_external_link_provider_symbol_classes(
    target_triple: str = "wasm32-wasip1",
) -> Mapping[str, str]:
    """Map every available provider export to its canonical provider class."""

    key = _provider_resolution_key(target_triple)
    classes = _provider_symbol_classes_from_key(key)
    _verify_provider_resolution_key(key)
    return classes


@functools.lru_cache(maxsize=8)
def _provider_symbol_classes_from_key(
    key: _ProviderResolutionKey,
) -> Mapping[str, str]:
    classes: dict[str, str] = {}
    surfaces = {
        surface.primitive_class: surface for surface in _provider_surfaces_from_key(key)
    }
    for primitive_class in _PROVIDER_CLASS_PRECEDENCE:
        for symbol in surfaces[primitive_class].symbols:
            classes.setdefault(symbol, primitive_class)
    return MappingProxyType(classes)


def wasm_external_link_provider_symbols(
    *,
    primitive_classes: frozenset[str] | None = None,
    target_triple: str = "wasm32-wasip1",
) -> frozenset[str]:
    key = _provider_resolution_key(target_triple)
    symbols = _provider_symbols_from_key(
        key,
        None if primitive_classes is None else tuple(sorted(primitive_classes)),
    )
    _verify_provider_resolution_key(key)
    return symbols


@functools.lru_cache(maxsize=24)
def _provider_symbols_from_key(
    key: _ProviderResolutionKey,
    primitive_classes: tuple[str, ...] | None,
) -> frozenset[str]:
    included = None if primitive_classes is None else frozenset(primitive_classes)
    return frozenset(
        symbol
        for surface in _provider_surfaces_from_key(key)
        if included is None or surface.primitive_class in included
        for symbol in surface.symbols
    )
