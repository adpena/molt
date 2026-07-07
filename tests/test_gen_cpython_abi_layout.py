"""Tests for tools/gen_cpython_abi_layout.py — the single-authority layout gate.

The CPython-ABI standalone tier (libmolt_cpython_abi) exposes traditional
ob_refcnt/ob_type structs whose layout is authored ONCE in Rust repr(C) form at
runtime/molt-cpython-abi/src/abi_types.rs. This generator emits a _Static_assert
parity block from that authority, #included by the ABI Python.h, so a drift
between the C header and the Rust authority the dylib operates on fails to compile
(the D1 duplicate_authority poison's memory-unsafe half).

Covered:
  * the checked-in generated artifact is IN SYNC with the authority (--check green);
  * the Rust authority parser extracts the canonical object-model structs;
  * NEGATIVE CONTROL: mutating the authority (a new field / a reordered field)
    changes the emitted parity block, i.e. the generator actually tracks the
    authority rather than emitting a constant — a gate that cannot detect drift
    is theater.
"""

from __future__ import annotations

import importlib.util
import sys
from pathlib import Path

import pytest

ROOT = Path(__file__).resolve().parents[1]
GEN = ROOT / "tools" / "gen_cpython_abi_layout.py"


def _load():
    spec = importlib.util.spec_from_file_location("molt_test_gen_abi_layout", GEN)
    assert spec is not None and spec.loader is not None
    mod = importlib.util.module_from_spec(spec)
    sys.modules["molt_test_gen_abi_layout"] = mod
    spec.loader.exec_module(mod)
    return mod


def test_check_is_green_on_checked_in_artifact() -> None:
    """The committed _molt_abi_layout.generated.h matches what the authority
    produces right now. If abi_types.rs was edited without regenerating, this
    fails — exactly the drift the gate exists to catch."""
    gen = _load()
    assert gen.main(["--check"]) == 0, (
        "generated layout artifact is stale vs abi_types.rs authority; run "
        "`python tools/gen_cpython_abi_layout.py --write`"
    )


def test_authority_parser_extracts_object_model_structs() -> None:
    gen = _load()
    authority = gen.parse_rust_authority(gen.AUTHORITY_RS.read_text(encoding="utf-8"))
    # The parser must find the object-model spine, with correct field categories.
    assert "PyObject" in authority
    pyobj = authority["PyObject"]
    assert [f.name for f in pyobj.fields] == ["ob_refcnt", "ob_type"]
    assert [f.category for f in pyobj.fields] == [gen.WORD, gen.PTR]
    # PyTypeObject is the big one; it must parse every field (embedded ob_base +
    # 49 tp_* fields = 50) without dropping the multi-line Option<fn> fields.
    tp = authority["PyTypeObject"]
    assert len(tp.fields) == 50, [f.name for f in tp.fields]
    assert tp.fields[0].name == "ob_base"
    assert tp.fields[-1].name == "tp_watched"
    assert tp.fields[-1].category == gen.BYTE


def test_generated_header_pins_pyobject_and_pytypeobject_size() -> None:
    gen = _load()
    text = gen.build()
    # PyObject is 2 words on LP64/LLP64 native (ob_refcnt + ob_type).
    assert "sizeof(PyObject) == 16u" in text
    # PyTypeObject matches CPython 3.12 native size.
    assert "sizeof(PyTypeObject) == 416u" in text
    assert "offsetof(PyTypeObject, tp_watched) == 408u" in text
    assert gen.GENERATED_BANNER in text


def test_negative_control_new_field_changes_emitted_layout(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    """Inject a synthetic extra field into the PyObject authority struct and prove
    the emitted parity block reflects the larger struct. If the generator emitted a
    constant, this would not change — proving the gate tracks the real authority."""
    gen = _load()
    real = gen.AUTHORITY_RS.read_text(encoding="utf-8")
    mutated = real.replace(
        "    pub ob_type: *mut PyTypeObject,\n}",
        "    pub ob_type: *mut PyTypeObject,\n    pub _synthetic_extra: Py_ssize_t,\n}",
        1,
    )
    assert mutated != real, "failed to inject synthetic field into PyObject"

    authority = gen.parse_rust_authority(mutated)
    assert [f.name for f in authority["PyObject"].fields] == [
        "ob_refcnt",
        "ob_type",
        "_synthetic_extra",
    ]
    emitted = gen.emit_header(authority)
    # PyObject grew from 16 to 24 bytes; the parity assert must track it.
    assert "sizeof(PyObject) == 24u" in emitted
    assert "sizeof(PyObject) == 16u" not in emitted


def test_negative_control_reordered_field_changes_offsets() -> None:
    """Reordering fields must change the emitted offsets (order is layout)."""
    gen = _load()
    real = gen.AUTHORITY_RS.read_text(encoding="utf-8")
    # Swap ob_refcnt and ob_type in PyObject.
    mutated = real.replace(
        "    pub ob_refcnt: Py_ssize_t,\n"
        "\n"
        "    /// Pointer to the type object. Points into our static type registry.\n"
        "    pub ob_type: *mut PyTypeObject,",
        "    pub ob_type: *mut PyTypeObject,\n"
        "    pub ob_refcnt: Py_ssize_t,",
        1,
    )
    if mutated == real:
        pytest.skip("abi_types.rs PyObject shape changed; update this negative control")
    authority = gen.parse_rust_authority(mutated)
    emitted = gen.emit_header(authority)
    # After the swap, ob_type is at 0 and ob_refcnt at 8.
    assert "offsetof(PyObject, ob_type) == 0u" in emitted
    assert "offsetof(PyObject, ob_refcnt) == 8u" in emitted


if __name__ == "__main__":  # pragma: no cover
    sys.exit(pytest.main([__file__, "-q"]))
