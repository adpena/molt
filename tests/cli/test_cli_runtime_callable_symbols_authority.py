from __future__ import annotations

import inspect
from pathlib import Path
import os

import pytest

import molt.cli as cli
from molt.cli import runtime_callable_symbols, native_symbol_inspection

_RUNTIME_CALLABLE_SYMBOL_NAMES = (
    "_runtime_callable_symbols_digest",
    "_runtime_callable_symbols_file",
    "_stage_runtime_callable_symbols_for_native_codegen",
)


def test_cli_runtime_callable_symbols_authority_is_single_home() -> None:
    for name in _RUNTIME_CALLABLE_SYMBOL_NAMES:
        assert getattr(cli, name) is getattr(runtime_callable_symbols, name)

    cli_source = inspect.getsource(cli)
    for name in _RUNTIME_CALLABLE_SYMBOL_NAMES:
        assert f"def {name}(" not in cli_source


def test_native_callable_symbol_stage_excludes_raw_borrowed_intrinsics(
    monkeypatch, tmp_path: Path
) -> None:
    runtime_lib = tmp_path / "molt_runtime.lib"
    runtime_lib.write_bytes(b"runtime")

    def inspect_archive(path, *, target_triple, identity, requirement):
        assert path == runtime_lib
        assert target_triple == "x86_64-pc-windows-msvc"
        assert identity.sha256
        assert requirement.function_prefix == "molt_"
        assert "molt_type_of_borrowed" in requirement.excluded_functions
        symbols = frozenset(
            {
                "molt_len",
                "molt_type_of_borrowed",
                "molt_dict_getitem_borrowed",
                "molt_list_getitem_borrowed",
                "molt_tuple_getitem_borrowed",
            }
        )
        return native_symbol_inspection._NativeGlobalSymbolFacts(
            symbols, frozenset(), symbols, artifact_digest=identity.sha256
        )

    monkeypatch.setattr(
        native_symbol_inspection, "_native_archive_global_symbol_facts", inspect_archive
    )

    symbols_file, failure = runtime_callable_symbols._runtime_callable_symbols_file(
        runtime_lib, target_triple="x86_64-pc-windows-msvc"
    )

    assert failure is None
    assert symbols_file is not None
    assert ".callable_symbols.v3." in symbols_file.name
    assert symbols_file.read_text(encoding="utf-8") == "molt_len\n"


def test_callable_projection_cannot_reuse_same_size_restored_mtime(
    monkeypatch, tmp_path: Path
) -> None:
    runtime_lib = tmp_path / "runtime.a"
    runtime_lib.write_bytes(b"first")
    stamp = runtime_lib.stat()

    def inspect_archive(path, *, target_triple, identity, requirement):
        symbol = "molt_" + path.read_text()
        symbols = frozenset({symbol})
        return native_symbol_inspection._NativeGlobalSymbolFacts(
            symbols, frozenset(), symbols, artifact_digest=identity.sha256
        )

    monkeypatch.setattr(
        native_symbol_inspection, "_native_archive_global_symbol_facts", inspect_archive
    )
    first, failure = runtime_callable_symbols._runtime_callable_symbols_file(
        runtime_lib
    )
    assert failure is None and first is not None
    runtime_lib.write_bytes(b"later")
    os.utime(runtime_lib, ns=(stamp.st_atime_ns, stamp.st_mtime_ns))
    second, failure = runtime_callable_symbols._runtime_callable_symbols_file(
        runtime_lib
    )
    assert failure is None and second is not None
    assert second != first
    assert second.read_text() == "molt_later\n"
    second.write_text("corrupt\n")
    repaired, failure = runtime_callable_symbols._runtime_callable_symbols_file(
        runtime_lib
    )
    assert failure is None and repaired == second
    assert second.read_text() == "molt_later\n"


@pytest.mark.parametrize("failure", ["unreadable", "changed"])
def test_callable_projection_preserves_shared_reader_failures(
    monkeypatch, tmp_path: Path, failure: str
) -> None:
    runtime_lib = tmp_path / "runtime.a"
    runtime_lib.write_bytes(b"runtime")

    def inspect_archive(path, **kwargs):
        raise native_symbol_inspection.NativeSymbolInspectionError(path, [failure])

    monkeypatch.setattr(
        native_symbol_inspection, "_native_archive_global_symbol_facts", inspect_archive
    )
    path, diagnostic = runtime_callable_symbols._runtime_callable_symbols_file(
        runtime_lib
    )
    assert path is None
    assert diagnostic is not None and failure in diagnostic
    assert not list(tmp_path.glob("*.callable_symbols.*"))
