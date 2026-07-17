from __future__ import annotations

import subprocess
from pathlib import Path

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
        return subprocess.CompletedProcess(
            args=["llvm-nm"],
            returncode=0,
            stdout="00000000 T exit\n         U fd_write\n",
            stderr="",
        )

    monkeypatch.setattr(backend_cache, "_default_molt_cache", lambda: cache_root)
    monkeypatch.setattr(
        backend_cache,
        "_native_object_global_symbols_result",
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

    def read_symbols(path: Path):
        reads.append(path)
        return facts[path]

    monkeypatch.setattr(
        providers,
        "_native_archive_global_symbol_sets",
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
    monkeypatch.setattr(
        providers,
        "_native_archive_global_symbol_sets",
        lambda _path: None,
    )
    providers._provider_surfaces_from_key.cache_clear()
    providers._provider_symbol_classes_from_key.cache_clear()
    providers._provider_symbols_from_key.cache_clear()

    assert providers.wasm_external_link_provider_symbol_classes() == {}
