from __future__ import annotations

import inspect
from pathlib import Path
from types import SimpleNamespace

import molt.cli as cli
from molt.cli import runtime_callable_symbols

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
    monkeypatch.setattr(
        runtime_callable_symbols, "_nm_candidate_binaries", lambda: ["nm"]
    )
    monkeypatch.setattr(
        runtime_callable_symbols,
        "_run_completed_command",
        lambda *args, **kwargs: SimpleNamespace(
            returncode=0,
            stdout="\n".join(
                [
                    "00000000 T molt_len",
                    "00000001 T molt_type_of_borrowed",
                    "00000002 T molt_dict_getitem_borrowed",
                    "00000003 T molt_list_getitem_borrowed",
                    "00000004 T molt_tuple_getitem_borrowed",
                ]
            ),
            stderr="",
        ),
    )

    symbols_file, failure = runtime_callable_symbols._runtime_callable_symbols_file(
        runtime_lib
    )

    assert failure is None
    assert symbols_file is not None
    assert ".callable_symbols.v2." in symbols_file.name
    assert symbols_file.read_text(encoding="utf-8") == "molt_len\n"
