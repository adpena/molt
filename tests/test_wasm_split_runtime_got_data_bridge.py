"""Typed-fact regression tests for the split-runtime GOT data-symbol bridge.

The split app link resolves PIC extension CPython-ABI data relocations through
temporary app-local aliases. ``wasm-ld --emit-relocs`` retains a transaction-
local linking symbol table, and the Rust facts scanner maps each
``GOT.data.internal.*`` symbol to its exact immutable i32 global. The linker
then retargets those globals to the shared runtime's canonical addresses before
final publication strips the linking metadata.
"""

from __future__ import annotations

from pathlib import Path

import pytest

from molt import wasm_artifact
from molt._wasm_runtime_exports import wasm_cpython_abi_data_symbol_names
import wasm_link_format as _format
import wasm_link_operations as _operations
import wasm_link_runtime_data as _runtime_data


def _global_module(inits: list[int]) -> bytes:
    payload = bytearray(_format._write_varuint(len(inits)))
    for index, value in enumerate(inits):
        payload.extend((0x7F, 0x00, 0x41))  # immutable i32, i32.const
        payload.extend(
            _runtime_data._write_wasm32_address(value, symbol=f"fixture-{index}")
        )
        payload.append(0x0B)
    return _operations.build_sections([(6, bytes(payload))])


def _alias_plan(
    tmp_path: Path, *symbols: str
) -> _runtime_data.SplitRuntimeDataAliasPlan:
    return _runtime_data.SplitRuntimeDataAliasPlan(
        artifact=tmp_path / "split_runtime_data_aliases.wasm",
        required_symbols=tuple(symbols),
        symbol_sizes=tuple((symbol, 8) for symbol in symbols),
    )


def _facts(*rows: tuple[str, int, int | None, int, bool]) -> dict[str, object]:
    return {
        "linking_symbol_table_present": True,
        "split_runtime_got_data_globals": rows,
    }


def _global_addresses(data: bytes) -> list[int]:
    addresses: list[int] = []
    for global_ in wasm_artifact.parse_wasm_defined_globals(data):
        assert global_.i32_const is not None
        addresses.append(global_.i32_const & 0xFFFF_FFFF)
    return addresses


def test_retargets_typed_got_facts_without_debug_name_section(tmp_path: Path) -> None:
    placeholder = 74_939_489
    runtime_addresses = {
        "Py_None": 4_092_912,
        "_Py_FalseStruct": 0x8000_0000,
    }
    data = _global_module([placeholder, placeholder, placeholder, 12_345])
    assert all(
        section_id != 0 for section_id, _payload in _operations.parse_sections(data)
    )
    canonical_symbols = wasm_cpython_abi_data_symbol_names()
    assert {"Py_None", "_Py_FalseStruct"}.issubset(canonical_symbols)

    rewritten, count = _runtime_data._rewrite_split_app_got_data_globals(
        data,
        runtime_addresses=runtime_addresses,
        alias_plan=_alias_plan(
            tmp_path,
            "molt_Py_None",
            "molt__Py_FalseStruct",
        ),
        wasm_facts=_facts(
            ("molt_Py_None", 0, placeholder, 0, True),
            ("molt__Py_FalseStruct", 1, placeholder, 0, True),
            ("numpy_local_thing", 2, placeholder, 0, True),
        ),
        description="TEST",
    )

    assert count == 2
    assert _global_addresses(rewritten) == [
        runtime_addresses["Py_None"],
        runtime_addresses["_Py_FalseStruct"],
        placeholder,
        12_345,
    ]
    before = dict(_operations.parse_sections(data))
    after = dict(_operations.parse_sections(rewritten))
    assert before.keys() == after.keys()
    assert all(
        before[section_id] == after[section_id]
        for section_id in before
        if section_id != 6
    )


def test_rewrite_fails_when_runtime_omits_attested_address(tmp_path: Path) -> None:
    placeholder = 74_939_489
    with pytest.raises(ValueError, match="Py_None"):
        _runtime_data._rewrite_split_app_got_data_globals(
            _global_module([placeholder]),
            runtime_addresses={},
            alias_plan=_alias_plan(tmp_path, "molt_Py_None"),
            wasm_facts=_facts(("molt_Py_None", 0, placeholder, 0, True)),
            description="TEST",
        )


def test_rewrite_requires_retained_linking_symbol_table(tmp_path: Path) -> None:
    with pytest.raises(ValueError, match="no retained linker symbol-table authority"):
        _runtime_data._rewrite_split_app_got_data_globals(
            _global_module([1]),
            runtime_addresses={"Py_None": 2},
            alias_plan=_alias_plan(tmp_path, "molt_Py_None"),
            wasm_facts={"split_runtime_got_data_globals": ()},
            description="TEST",
        )


def test_rewrite_rejects_stale_fact_initializer(tmp_path: Path) -> None:
    with pytest.raises(ValueError, match="stale initializer authority"):
        _runtime_data._rewrite_split_app_got_data_globals(
            _global_module([1]),
            runtime_addresses={"Py_None": 2},
            alias_plan=_alias_plan(tmp_path, "molt_Py_None"),
            wasm_facts=_facts(("molt_Py_None", 0, 7, 0, True)),
            description="TEST",
        )


def test_rewrite_rejects_missing_global_fact_target(tmp_path: Path) -> None:
    with pytest.raises(ValueError, match="imported or missing global"):
        _runtime_data._rewrite_split_app_got_data_globals(
            _global_module([1]),
            runtime_addresses={"Py_None": 2},
            alias_plan=_alias_plan(tmp_path, "molt_Py_None"),
            wasm_facts=_facts(("molt_Py_None", 7, 1, 0, True)),
            description="TEST",
        )


def test_facts_reject_duplicate_symbol_and_global_rows() -> None:
    with pytest.raises(ValueError, match="duplicate split app GOT data symbol"):
        _runtime_data._split_runtime_got_data_global_facts(
            _facts(("molt_Py_None", 0, 1, 0, True), ("molt_Py_None", 1, 2, 0, True))
        )
    with pytest.raises(ValueError, match="duplicate split app GOT data global index"):
        _runtime_data._split_runtime_got_data_global_facts(
            _facts(("molt_Py_None", 0, 1, 0, True), ("molt_PyList_Type", 0, 2, 0, True))
        )


def test_empty_attested_got_set_is_a_non_pic_noop(tmp_path: Path) -> None:
    data = _global_module([74_939_489])
    rewritten, count = _runtime_data._rewrite_split_app_got_data_globals(
        data,
        runtime_addresses={"Py_None": 4_092_912},
        alias_plan=_alias_plan(tmp_path, "molt_Py_None"),
        wasm_facts=_facts(),
        description="TEST",
    )
    assert (rewritten, count) == (data, 0)


def _rust_type_static_ptr_names() -> set[str]:
    """Return the Py*_Type identifiers registered by type_static_ptrs()."""
    import re

    root = Path(__file__).resolve().parents[1]
    source = (root / "runtime" / "molt-cpython-abi" / "src" / "abi_types.rs").read_text(
        encoding="utf-8"
    )
    marker = "pub fn type_static_ptrs() -> Vec<*mut PyObject> {"
    start = source.index(marker)
    body = source[start : source.index("\n}", start)]
    return set(re.findall(r"&raw mut (\w+) as \*mut PyObject", body))


def test_rust_type_static_ptrs_match_split_runtime_data_symbol_authority() -> None:
    authority = {
        name for name in wasm_cpython_abi_data_symbol_names() if name.endswith("Type")
    }
    assert _rust_type_static_ptr_names() == authority


@pytest.mark.parametrize("flags, defined", [(1, True), (2, True), (0x10, False)])
def test_unrelated_nonexact_got_binding_is_ignored(
    tmp_path: Path, flags: int, defined: bool
) -> None:
    data = _global_module([1])
    rewritten, count = _runtime_data._rewrite_split_app_got_data_globals(
        data,
        runtime_addresses={},
        alias_plan=_alias_plan(tmp_path),
        wasm_facts=_facts(("numpy_local_thing", 0, None, flags, defined)),
        description="TEST",
    )
    assert (rewritten, count) == (data, 0)


@pytest.mark.parametrize(
    "flags, defined, message",
    [
        (1, True, "exact global binding"),
        (2, True, "exact global binding"),
        (0x10, False, "is undefined"),
    ],
)
def test_selected_cpython_got_binding_must_be_exact(
    tmp_path: Path, flags: int, defined: bool, message: str
) -> None:
    with pytest.raises(ValueError, match=message):
        _runtime_data._rewrite_split_app_got_data_globals(
            _global_module([1]),
            runtime_addresses={"Py_None": 2},
            alias_plan=_alias_plan(tmp_path, "molt_Py_None"),
            wasm_facts=_facts(("molt_Py_None", 0, 1, flags, defined)),
            description="TEST",
        )


@pytest.mark.parametrize(
    "value_type, mutable, message",
    [(0x7E, False, "non-i32"), (0x7F, True, "mutable global")],
)
def test_selected_cpython_got_global_shape_is_checked(
    tmp_path: Path, value_type: int, mutable: bool, message: str
) -> None:
    data = _operations.build_sections(
        [
            (
                6,
                bytes(
                    [
                        1,
                        value_type,
                        int(mutable),
                        0x42 if value_type == 0x7E else 0x41,
                        1,
                        0x0B,
                    ]
                ),
            )
        ]
    )
    with pytest.raises(ValueError, match=message):
        _runtime_data._rewrite_split_app_got_data_globals(
            data,
            runtime_addresses={"Py_None": 2},
            alias_plan=_alias_plan(tmp_path, "molt_Py_None"),
            wasm_facts=_facts(("molt_Py_None", 0, 1, 0, True)),
            description="TEST",
        )
