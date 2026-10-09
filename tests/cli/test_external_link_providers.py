from __future__ import annotations

from pathlib import Path
from contextlib import contextmanager
from collections import OrderedDict
import os
import sys

import pytest

from molt.cli import external_link_providers as providers
from molt.cli import native_symbol_inspection
from tests.cli.native_link_test_support import (
    single_member_archive_symbol_facts,
    static_archive_bytes,
)
from tests.process_guard_common import install_module_view


def test_archive_symbol_facts_use_central_cache_without_toolchain_sidecar(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    archive = tmp_path / "toolchain" / "libc.a"
    archive.parent.mkdir()
    archive.write_bytes(static_archive_bytes())
    cache_root = tmp_path / "cache"
    reads = 0

    def read_symbols(*_args, archive_members, **_kwargs):
        nonlocal reads
        reads += 1
        return single_member_archive_symbol_facts(
            archive_members,
            native_symbol_inspection._NativeGlobalSymbolFacts(
                defined=frozenset({"exit"}),
                undefined=frozenset({"fd_write"}),
                defined_functions=frozenset({"exit"}),
            ),
        )

    monkeypatch.setattr(
        native_symbol_inspection, "_default_molt_cache", lambda: cache_root
    )
    monkeypatch.setattr(
        native_symbol_inspection,
        "_read_native_global_symbol_facts",
        read_symbols,
    )
    native_symbol_inspection._NATIVE_ARCHIVE_SYMBOL_SETS_CACHE.clear()

    assert native_symbol_inspection._native_archive_global_symbol_sets(archive) == (
        {"exit"},
        {"fd_write"},
    )
    assert reads == 1
    assert not archive.with_suffix(".symbols.json").exists()

    native_symbol_inspection._NATIVE_ARCHIVE_SYMBOL_SETS_CACHE.clear()
    assert native_symbol_inspection._native_archive_global_symbol_sets(archive) == (
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
    compiler_rt = tmp_path / "libclang_rt.builtins.a"
    libcxx = tmp_path / "libc++.a"
    libcxxabi = tmp_path / "libc++abi.a"
    libunwind = tmp_path / "libunwind.a"
    for path in (libc, compiler_rt, libcxx, libcxxabi, libunwind):
        path.write_bytes(path.name.encode("ascii"))

    monkeypatch.setattr(
        providers,
        "_resolved_provider_archives",
        lambda _target, _classes, _plan: (
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

    @contextmanager
    def read_symbols(path: Path, *, target_triple: str, archive: bool):
        assert archive
        identity = native_symbol_inspection._native_symbol_artifact_identity(path)
        reads.append(path)
        assert target_triple == "wasm32-wasip1"
        defined, undefined = facts[path]
        yield (
            None,
            identity,
            native_symbol_inspection._NativeGlobalSymbolFacts(
                defined=frozenset(defined),
                undefined=frozenset(undefined),
                defined_functions=frozenset(defined),
                artifact_digest=identity.sha256,
            ),
        )

    monkeypatch.setattr(
        providers,
        "_native_symbol_facts_admission",
        read_symbols,
    )

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
    assert providers.wasm_external_link_provider_symbol_classes() == classes
    assert set(reads) == set(facts), "projections always enter canonical admission"


def test_unreadable_provider_family_fails_closed(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    libc = tmp_path / "libc.a"
    libc.write_bytes(b"archive")
    monkeypatch.setattr(
        providers,
        "_resolved_provider_archives",
        lambda _target, _classes, _plan: (
            (providers.WASM_LIBC_LINK_IMPORT_CLASS, (libc,)),
            (providers.WASM_COMPILER_RT_LINK_IMPORT_CLASS, ()),
            (providers.WASM_LIBCXX_LINK_IMPORT_CLASS, ()),
        ),
    )

    @contextmanager
    def unreadable(path: Path, *, target_triple: str, archive: bool):
        raise native_symbol_inspection.NativeSymbolInspectionError(
            path, ["provider unreadable"]
        )
        yield

    monkeypatch.setattr(providers, "_native_symbol_facts_admission", unreadable)

    for _ in range(2):
        with pytest.raises(
            native_symbol_inspection.NativeSymbolInspectionError,
            match="provider unreadable",
        ):
            providers.wasm_external_link_provider_symbol_classes()


def test_nm_symbol_normalization_uses_artifact_target_not_host(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    output = "00000000 T __molt_runtime\n         U _Py_None\n"
    install_module_view(
        monkeypatch, "sys", sys, native_symbol_inspection, platform="darwin"
    )

    wasm_defined, wasm_undefined = (
        native_symbol_inspection._parse_native_nm_global_symbol_sets(
            output,
            target_triple="wasm32-wasip1",
        )
    )
    macho_defined, macho_undefined = (
        native_symbol_inspection._parse_native_nm_global_symbol_sets(
            output,
            target_triple="aarch64-apple-darwin",
        )
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
def test_provider_projection_admits_once_and_fences_native_cache_hits(
    tmp_path, monkeypatch, query
):
    archive = tmp_path / "libc.a"
    archive.write_bytes(static_archive_bytes(b"original"))
    monkeypatch.setattr(
        providers,
        "_resolved_provider_archives",
        lambda _target, _classes, _plan: (
            (providers.WASM_LIBC_LINK_IMPORT_CLASS, (archive,)),
            (providers.WASM_COMPILER_RT_LINK_IMPORT_CLASS, ()),
            (providers.WASM_LIBCXX_LINK_IMPORT_CLASS, ()),
        ),
    )
    monkeypatch.setattr(
        native_symbol_inspection, "_default_molt_cache", lambda: tmp_path / "cache"
    )

    reader_path = tmp_path / "llvm-nm"
    reader_path.write_bytes(b"test reader")
    reader_identity = native_symbol_inspection.stable_regular_file_identity(
        reader_path, label="test llvm-nm"
    )
    requirement = native_symbol_inspection.NativeSymbolRequirement()
    reader = native_symbol_inspection._NativeSymbolReader(
        candidates=(
            native_symbol_inspection._NativeSymbolReaderCandidate(
                (str(reader_path),),
                executable_identity=reader_identity,
                reader_family="llvm",
            ),
        ),
        input_identity=(
            "outer-provider-cache-test-reader",
            str(reader_path),
            reader_identity.sha256,
            requirement.cache_identity(),
        ),
        requirement=requirement,
    )

    def symbol_reader(*, nm_command, target_triple, requirement):
        assert nm_command is None
        assert target_triple == "wasm32-wasip1"
        assert requirement == reader.requirement
        return reader

    monkeypatch.setattr(
        native_symbol_inspection, "_native_symbol_reader", symbol_reader
    )

    def read_symbols(*_args, archive_members, _reader, **_kwargs):
        assert _reader is reader
        return single_member_archive_symbol_facts(
            archive_members,
            native_symbol_inspection._NativeGlobalSymbolFacts(
                defined=frozenset({"exit"}),
                undefined=frozenset(),
                defined_functions=frozenset({"exit"}),
            ),
        )

    monkeypatch.setattr(
        native_symbol_inspection, "_read_native_global_symbol_facts", read_symbols
    )
    native_symbol_inspection._NATIVE_ARCHIVE_SYMBOL_SETS_CACHE.clear()
    captured = []
    original_hash = native_symbol_inspection.stable_regular_file_handle_identity

    def capture(opened, **kwargs):
        captured.append(opened.path)
        return original_hash(opened, **kwargs)

    monkeypatch.setattr(
        native_symbol_inspection, "stable_regular_file_handle_identity", capture
    )
    query()
    assert captured == [archive]
    captured.clear()
    query()
    assert captured == [archive]
    storage = native_symbol_inspection._NATIVE_ARCHIVE_SYMBOL_SETS_CACHE

    class ReplacingCache(OrderedDict):
        def get(self, key, default=None):
            found = super().get(key, default)
            assert found is not None
            stamp = archive.stat()
            archive.write_bytes(static_archive_bytes(b"replacement-with-changed-size"))
            os.utime(archive, ns=(stamp.st_atime_ns, stamp.st_mtime_ns))
            return found

    monkeypatch.setattr(
        native_symbol_inspection,
        "_NATIVE_ARCHIVE_SYMBOL_SETS_CACHE",
        ReplacingCache(storage),
    )
    with pytest.raises(
        native_symbol_inspection.NativeSymbolInspectionError, match="changed"
    ):
        query()


def test_resolved_c_provider_families_share_the_selected_sdk(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    from tests.runtime_build_identity_helper import (
        RuntimeFixtureRoot,
        runtime_wasi_c_abi_plan,
    )

    plan = runtime_wasi_c_abi_plan(RuntimeFixtureRoot(tmp_path))
    cxx = plan.sysroot / "lib/wasm32-wasip1/eh"
    cxx.mkdir()
    archives = tuple(cxx / name for name in ("libc++.a", "libc++abi.a", "libunwind.a"))
    for path in archives:
        path.write_bytes(b"!<arch>\n")
    monkeypatch.setattr(
        providers.wasm_link_inputs, "resolve_wasi_c_abi_plan", lambda **kw: plan
    )
    assert providers._resolved_provider_archives("wasm32-wasip1", None, None) == (
        (
            providers.WASM_LIBC_LINK_IMPORT_CLASS,
            (plan.path("long_double"), plan.path("libc")),
        ),
        (providers.WASM_COMPILER_RT_LINK_IMPORT_CLASS, (plan.path("compiler_rt"),)),
        (providers.WASM_LIBCXX_LINK_IMPORT_CLASS, archives),
    )
    monkeypatch.setattr(
        providers.wasm_link_inputs,
        "resolve_wasi_c_abi_plan",
        lambda **kw: pytest.fail("freestanding source extension must not select libc"),
    )
    assert all(
        not paths
        for _, paths in providers._resolved_provider_archives(
            "wasm32-unknown-unknown", None, None
        )
    )


def test_compiler_rt_discovery_does_not_resolve_unrequested_cxx(monkeypatch, tmp_path):
    from tests.runtime_build_identity_helper import (
        RuntimeFixtureRoot,
        runtime_wasi_c_abi_plan,
    )

    plan = runtime_wasi_c_abi_plan(RuntimeFixtureRoot(tmp_path))
    monkeypatch.setattr(
        providers.wasm_link_inputs,
        "wasm_cxx_runtime_archives",
        lambda **kwargs: pytest.fail("unrequested C++ family inspected"),
    )
    assert providers._resolved_provider_archives(
        "wasm32-wasip1", frozenset({providers.WASM_COMPILER_RT_LINK_IMPORT_CLASS}), plan
    ) == ((providers.WASM_COMPILER_RT_LINK_IMPORT_CLASS, (plan.path("compiler_rt"),)),)
