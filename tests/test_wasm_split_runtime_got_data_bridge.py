"""Regression tests for the split-runtime GOT data-symbol bridge.

The split app link resolves a PIC extension's (numpy ``_multiarray_umath``)
CPython-ABI data relocations (``Py_None``/``Py_False``/``PyExc_*``/``Py*_Type``)
to app-local placeholder addresses via the active-data-segment alias, materialised
by wasm-ld as defined ``GOT.data.internal.molt_<sym>`` globals. Those globals are
the exact word the extension reads at run time, so ``wasm_link`` retargets each to
the shared runtime's canonical linear-memory address after the link. These tests
lock in the retarget + its fail-loud contract without a full wasm build.
"""

from __future__ import annotations

import importlib.util
from pathlib import Path

import pytest


def _load_wasm_link():
    root = Path(__file__).resolve().parents[1]
    path = root / "tools" / "wasm_link.py"
    spec = importlib.util.spec_from_file_location("molt_wasm_link_got", path)
    module = importlib.util.module_from_spec(spec)
    assert spec.loader is not None
    spec.loader.exec_module(module)
    return module


wasm_link = _load_wasm_link()


def _global_section(inits: list[int]) -> bytes:
    payload = bytearray()
    payload.extend(wasm_link._write_varuint(len(inits)))
    for value in inits:
        payload.append(0x7F)  # i32
        payload.append(0x01)  # mutable (GOT globals are mutable)
        payload.append(0x41)  # i32.const
        payload.extend(wasm_link._write_sleb128(value))
        payload.append(0x0B)  # end
    return bytes(payload)


def _name_section(global_names: dict[int, str]) -> bytes:
    sub = bytearray()
    sub.extend(wasm_link._write_varuint(len(global_names)))
    for index, name in sorted(global_names.items()):
        sub.extend(wasm_link._write_varuint(index))
        sub.extend(wasm_link._write_string(name))
    body = bytearray()
    body.append(7)  # global names subsection id
    body.extend(wasm_link._write_varuint(len(sub)))
    body.extend(sub)
    return wasm_link._build_custom_section("name", bytes(body))


def _module(inits: list[int], global_names: dict[int, str]) -> bytes:
    return wasm_link._build_sections(
        [
            (6, _global_section(inits)),
            (0, _name_section(global_names)),
        ]
    )


def test_write_sleb128_roundtrips():
    for value in (0, 1, -1, 63, 64, 4092912, 74939489, 0x7FFFFFFF):
        encoded = wasm_link._write_sleb128(value)
        decoded, offset = wasm_link._read_varsint(encoded, 0)
        assert decoded == value
        assert offset == len(encoded)


def test_name_section_global_names_roundtrip():
    names = {0: "GOT.data.internal.molt_Py_None", 3: "some_other_global"}
    data = _module([0, 0, 0, 0], names)
    assert wasm_link._wasm_name_section_global_names(data) == names


def test_retargets_cpython_abi_got_data_globals_to_runtime_addresses():
    placeholder = 74939489
    # Two runtime-owned CPython-ABI data symbols + one unrelated global that must
    # be left untouched.
    canonical_symbols = wasm_link.wasm_cpython_abi_data_symbol_names()
    assert "Py_None" in canonical_symbols and "Py_False" in canonical_symbols
    names = {
        0: "GOT.data.internal.molt_Py_None",
        1: "GOT.data.internal.molt_Py_False",
        2: "GOT.data.internal.numpy_local_thing",  # not runtime-owned -> untouched
        3: "app_scratch_global",
    }
    data = _module([placeholder, placeholder, placeholder, 12345], names)
    runtime_addresses = {"Py_None": 4092912, "Py_False": 4092904}

    new_data, count = wasm_link._rewrite_split_app_got_data_globals(
        data, runtime_addresses=runtime_addresses, description="TEST"
    )
    assert count == 2
    inits = wasm_link._defined_global_i32_inits(new_data)
    assert inits[0] == 4092912  # molt_Py_None -> runtime Py_None
    assert inits[1] == 4092904  # molt_Py_False -> runtime Py_False
    assert inits[2] == placeholder  # numpy-local GOT global untouched
    assert inits[3] == 12345  # unrelated global untouched
    # Only the global section changed.
    before = dict(wasm_link._parse_sections(data))
    after = dict(wasm_link._parse_sections(new_data))
    assert before.keys() == after.keys()
    for section_id in before:
        if section_id == 6:
            continue
        assert before[section_id] == after[section_id]


def test_fails_loud_when_runtime_publishes_no_address_for_required_symbol():
    placeholder = 74939489
    names = {0: "GOT.data.internal.molt_Py_None"}
    data = _module([placeholder], names)
    # Runtime address map is missing Py_None -> must not silently leave the
    # extension reading the app-local placeholder.
    with pytest.raises(ValueError, match="Py_None"):
        wasm_link._rewrite_split_app_got_data_globals(
            data, runtime_addresses={}, description="TEST"
        )


def test_noop_when_no_name_section():
    data = wasm_link._build_sections([(6, _global_section([74939489]))])
    new_data, count = wasm_link._rewrite_split_app_got_data_globals(
        data, runtime_addresses={"Py_None": 4092912}, description="TEST"
    )
    assert count == 0
    assert new_data == data


def _rust_type_static_ptr_names() -> set[str]:
    """The `Py*_Type` identifiers registered by `abi_types::type_static_ptrs()`.

    Parsed from the runtime source so the drift gate below binds the Rust bridge
    registration list to the Python split-runtime export authority.
    """
    import re

    root = Path(__file__).resolve().parents[1]
    src = (root / "runtime" / "molt-cpython-abi" / "src" / "abi_types.rs").read_text(
        encoding="utf-8"
    )
    marker = "pub fn type_static_ptrs() -> Vec<*mut PyObject> {"
    start = src.index(marker)
    body = src[start : src.index("\n}", start)]
    return set(re.findall(r"&raw mut (\w+) as \*mut PyObject", body))


def test_rust_type_static_ptrs_match_split_runtime_data_symbol_authority():
    """M45 drift gate: the Rust `type_static_ptrs()` bridge-registration list must
    stay in lock-step with the `*_Type` canonical data symbols the split-runtime
    exports (`wasm_cpython_abi_data_symbol_names`). A one-symbol drift means a
    builtin type static handed back by numpy fails `pyobj_to_handle` (the
    `_multiarray_umath` `PyDict_SetItem(unresolved key)` class) or a stale entry
    lingers after a type static is removed.
    """
    authority = {
        name
        for name in wasm_link.wasm_cpython_abi_data_symbol_names()
        if name.endswith("Type")
    }
    assert _rust_type_static_ptr_names() == authority
