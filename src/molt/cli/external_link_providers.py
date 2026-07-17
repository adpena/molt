from __future__ import annotations

import functools
from dataclasses import dataclass
from pathlib import Path
from types import MappingProxyType
from typing import Mapping

from molt.cli import wasm_toolchain
from molt.cli.backend_cache import _native_archive_global_symbol_sets


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
) -> tuple[tuple[str, tuple[Path, ...]], ...]:
    if target_triple == "wasm32-wasip1":
        libc = wasm_toolchain.wasm_wasi_libc_archive()
        compiler_rt = wasm_toolchain.wasm_compiler_builtins_archive()
        libcxx = wasm_toolchain.wasm_cxx_runtime_archives()
    else:
        libc = wasm_toolchain.wasm_wasi_libc_archive(target_triple)
        compiler_rt = wasm_toolchain.wasm_compiler_builtins_archive(target_triple)
        libcxx = wasm_toolchain.wasm_cxx_runtime_archives(target_triple)
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
) -> tuple[tuple[str, tuple[tuple[str, int, int, int], ...]], ...]:
    key: list[tuple[str, tuple[tuple[str, int, int, int], ...]]] = []
    for primitive_class, archives in _resolved_provider_archives(target_triple):
        archive_keys: list[tuple[str, int, int, int]] = []
        for archive in archives:
            try:
                resolved = archive.resolve(strict=True)
                stat = resolved.stat()
            except OSError:
                archive_keys.append((str(archive.resolve(strict=False)), -1, -1, -1))
                continue
            archive_keys.append(
                (
                    str(resolved),
                    int(stat.st_size),
                    int(stat.st_mtime_ns),
                    int(getattr(stat, "st_ctime_ns", 0)),
                )
            )
        key.append((primitive_class, tuple(archive_keys)))
    return tuple(key)


@functools.lru_cache(maxsize=8)
def _provider_surfaces_from_key(
    key: tuple[tuple[str, tuple[tuple[str, int, int, int], ...]], ...],
) -> tuple[ExternalLinkProviderSurface, ...]:
    surfaces: list[ExternalLinkProviderSurface] = []
    for primitive_class, archive_keys in key:
        archives = tuple(Path(path) for path, _size, _mtime, _ctime in archive_keys)
        symbols: set[str] = set()
        readable = bool(archives)
        for archive in archives:
            facts = _native_archive_global_symbol_sets(archive)
            if facts is None:
                readable = False
                break
            defined, _undefined = facts
            symbols.update(defined)
        surfaces.append(
            ExternalLinkProviderSurface(
                primitive_class=primitive_class,
                archives=archives,
                symbols=frozenset(symbols if readable else ()),
            )
        )
    return tuple(surfaces)


def wasm_external_link_provider_surfaces(
    target_triple: str = "wasm32-wasip1",
) -> tuple[ExternalLinkProviderSurface, ...]:
    """Return exact symbols owned by the archives the final linker will stage.

    This is the canonical external-native libc/compiler-rt/libc++ authority.
    It reads the resolved archive symbol tables directly, so upgrading a
    toolchain cannot silently retain a stale hand-maintained subset.  Missing or
    unreadable provider families expose an empty surface and therefore fail
    closed at the existing undefined-symbol custody audit.
    """

    return _provider_surfaces_from_key(_provider_resolution_key(target_triple))


def wasm_external_link_provider_symbol_classes(
    target_triple: str = "wasm32-wasip1",
) -> Mapping[str, str]:
    """Map every available provider export to its canonical provider class."""

    return _provider_symbol_classes_from_key(
        _provider_resolution_key(target_triple)
    )


@functools.lru_cache(maxsize=8)
def _provider_symbol_classes_from_key(
    key: tuple[tuple[str, tuple[tuple[str, int, int, int], ...]], ...],
) -> Mapping[str, str]:
    classes: dict[str, str] = {}
    surfaces = {
        surface.primitive_class: surface
        for surface in _provider_surfaces_from_key(key)
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
    return _provider_symbols_from_key(
        _provider_resolution_key(target_triple),
        None if primitive_classes is None else tuple(sorted(primitive_classes)),
    )


@functools.lru_cache(maxsize=24)
def _provider_symbols_from_key(
    key: tuple[tuple[str, tuple[tuple[str, int, int, int], ...]], ...],
    primitive_classes: tuple[str, ...] | None,
) -> frozenset[str]:
    included = None if primitive_classes is None else frozenset(primitive_classes)
    return frozenset(
        symbol
        for surface in _provider_surfaces_from_key(key)
        if included is None or surface.primitive_class in included
        for symbol in surface.symbols
    )
