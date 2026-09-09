from __future__ import annotations

from pathlib import Path
import os

import pytest

from molt.cli import external_link_providers as providers
from molt.cli import backend_cache


def test_archive_symbol_facts_use_central_cache_without_toolchain_sidecar(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    archive = tmp_path / "toolchain" / "libc.a"
    archive.parent.mkdir()
    archive.write_bytes(b"!<arch>\n")
    cache_root = tmp_path / "cache"
    reads = 0

    def read_symbols(*_args, **_kwargs):
        nonlocal reads
        reads += 1
        return backend_cache._NativeGlobalSymbolFacts(
            defined=frozenset({"exit"}),
            undefined=frozenset({"fd_write"}),
            defined_functions=frozenset({"exit"}),
        )

    monkeypatch.setattr(backend_cache, "_default_molt_cache", lambda: cache_root)
    monkeypatch.setattr(
        backend_cache,
        "_read_native_global_symbol_facts",
        read_symbols,
    )
    backend_cache._NATIVE_ARCHIVE_SYMBOL_SETS_CACHE.clear()

    assert backend_cache._native_archive_global_symbol_sets(archive) == (
        {"exit"},
        {"fd_write"},
    )
    assert reads == 1
    assert not archive.with_suffix(".symbols.json").exists()

    backend_cache._NATIVE_ARCHIVE_SYMBOL_SETS_CACHE.clear()
    assert backend_cache._native_archive_global_symbol_sets(archive) == (
        {"exit"},
        {"fd_write"},
    )
    assert reads == 1
    assert len(list(cache_root.rglob("*.json"))) == 1


def test_provider_surface_owns_complete_archive_symbol_families(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    libc = tmp_path / "libc.a"
    compiler_rt = tmp_path / "libcompiler_builtins.rlib"
    libcxx = tmp_path / "libc++.a"
    libcxxabi = tmp_path / "libc++abi.a"
    libunwind = tmp_path / "libunwind.a"
    for path in (libc, compiler_rt, libcxx, libcxxabi, libunwind):
        path.write_bytes(path.name.encode("ascii"))

    monkeypatch.setattr(
        providers,
        "_resolved_provider_archives",
        lambda _target: (
            (providers.WASM_LIBC_LINK_IMPORT_CLASS, (libc,)),
            (providers.WASM_COMPILER_RT_LINK_IMPORT_CLASS, (compiler_rt,)),
            (
                providers.WASM_LIBCXX_LINK_IMPORT_CLASS,
                (libcxx, libcxxabi, libunwind),
            ),
        ),
    )
    facts = {
        libc: ({"exit", "putc", "shared"}, set()),
        compiler_rt: ({"__trunctfdf2", "shared"}, set()),
        libcxx: ({"_Znwm"}, set()),
        libcxxabi: ({"_ZdaPv", "_Znam"}, set()),
        libunwind: ({"_Unwind_RaiseException"}, set()),
    }
    reads: list[Path] = []

    def read_symbols(path: Path, *, target_triple: str, identity):
        reads.append(path)
        assert target_triple == "wasm32-wasip1"
        defined, undefined = facts[path]
        return backend_cache._NativeGlobalSymbolFacts(
            defined=frozenset(defined),
            undefined=frozenset(undefined),
            defined_functions=frozenset(defined),
            artifact_digest=identity.sha256,
        )

    monkeypatch.setattr(
        providers,
        "_native_archive_global_symbol_facts",
        read_symbols,
    )
    providers._provider_surfaces_from_key.cache_clear()
    providers._provider_symbol_classes_from_key.cache_clear()
    providers._provider_symbols_from_key.cache_clear()

    classes = providers.wasm_external_link_provider_symbol_classes()
    assert classes["exit"] == providers.WASM_LIBC_LINK_IMPORT_CLASS
    assert classes["putc"] == providers.WASM_LIBC_LINK_IMPORT_CLASS
    assert classes["__trunctfdf2"] == providers.WASM_COMPILER_RT_LINK_IMPORT_CLASS
    assert classes["_ZdaPv"] == providers.WASM_LIBCXX_LINK_IMPORT_CLASS
    assert classes["_Znam"] == providers.WASM_LIBCXX_LINK_IMPORT_CLASS
    assert classes["_Unwind_RaiseException"] == providers.WASM_LIBCXX_LINK_IMPORT_CLASS
    assert classes["shared"] == providers.WASM_LIBC_LINK_IMPORT_CLASS
    assert set(reads) == set(facts)

    reads.clear()
    assert providers.wasm_external_link_provider_symbol_classes() is classes
    assert reads == []


def test_unreadable_provider_family_fails_closed(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    libc = tmp_path / "libc.a"
    libc.write_bytes(b"archive")
    monkeypatch.setattr(
        providers,
        "_resolved_provider_archives",
        lambda _target: (
            (providers.WASM_LIBC_LINK_IMPORT_CLASS, (libc,)),
            (providers.WASM_COMPILER_RT_LINK_IMPORT_CLASS, ()),
            (providers.WASM_LIBCXX_LINK_IMPORT_CLASS, ()),
        ),
    )

    def unreadable(path: Path, *, target_triple: str, identity):
        raise backend_cache.NativeSymbolInspectionError(path, ["provider unreadable"])

    monkeypatch.setattr(providers, "_native_archive_global_symbol_facts", unreadable)
    providers._provider_surfaces_from_key.cache_clear()
    providers._provider_symbol_classes_from_key.cache_clear()
    providers._provider_symbols_from_key.cache_clear()

    for _ in range(2):
        with pytest.raises(
            backend_cache.NativeSymbolInspectionError, match="provider unreadable"
        ):
            providers.wasm_external_link_provider_symbol_classes()
        assert providers._provider_surfaces_from_key.cache_info().currsize == 0


def test_nm_symbol_normalization_uses_artifact_target_not_host(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    output = "00000000 T __molt_runtime\n         U _Py_None\n"
    monkeypatch.setattr(backend_cache.sys, "platform", "darwin")

    wasm_defined, wasm_undefined = backend_cache._parse_native_nm_global_symbol_sets(
        output,
        target_triple="wasm32-wasip1",
    )
    macho_defined, macho_undefined = backend_cache._parse_native_nm_global_symbol_sets(
        output,
        target_triple="aarch64-apple-darwin",
    )

    assert wasm_defined == {"__molt_runtime"}
    assert wasm_undefined == {"_Py_None"}
    assert macho_defined == {"_molt_runtime"}
    assert macho_undefined == {"Py_None"}


@pytest.mark.parametrize(
    "query",
    [
        providers.wasm_external_link_provider_surfaces,
        providers.wasm_external_link_provider_symbol_classes,
        providers.wasm_external_link_provider_symbols,
    ],
)
def test_outer_provider_cache_rechecks_generation_before_return(
    tmp_path, monkeypatch, query
):
    archive = tmp_path / "libc.a"
    archive.write_bytes(b"original")
    monkeypatch.setattr(
        providers,
        "_resolved_provider_archives",
        lambda target: (
            (providers.WASM_LIBC_LINK_IMPORT_CLASS, (archive,)),
            (providers.WASM_COMPILER_RT_LINK_IMPORT_CLASS, ()),
            (providers.WASM_LIBCXX_LINK_IMPORT_CLASS, ()),
        ),
    )
    monkeypatch.setattr(
        backend_cache, "_default_molt_cache", lambda: tmp_path / "cache"
    )
    monkeypatch.setattr(
        backend_cache,
        "_read_native_global_symbol_facts",
        lambda *args, **kwargs: backend_cache._NativeGlobalSymbolFacts(
            defined=frozenset({"exit"}),
            undefined=frozenset(),
            defined_functions=frozenset({"exit"}),
        ),
    )
    for cached in (
        providers._provider_surfaces_from_key,
        providers._provider_symbol_classes_from_key,
        providers._provider_symbols_from_key,
    ):
        cached.cache_clear()
    query()
    original = providers._provider_resolution_key

    def replace_after_key(target):
        key = original(target)
        stamp = archive.stat()
        archive.write_bytes(b"replaced")
        os.utime(archive, ns=(stamp.st_atime_ns, stamp.st_mtime_ns))
        return key

    monkeypatch.setattr(providers, "_provider_resolution_key", replace_after_key)
    with pytest.raises(backend_cache.NativeSymbolInspectionError, match="changed"):
        query()
