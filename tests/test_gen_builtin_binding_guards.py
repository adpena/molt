"""Drift and domain checks for the builtin binding guard authority (HF-79)."""

from __future__ import annotations

import builtins
import importlib
import re

import pytest

from molt.compiler_analysis.builtin_binding_guards_generated import (
    BUILTIN_BINDING_GUARD_ALL_MASK,
    BUILTIN_BINDING_GUARD_BITS,
)
from molt.compiler_analysis.python_builtin_shapes import BUILTIN_SHAPE_NAMES
from molt.frontend._types import BUILTIN_FUNC_SPECS, BUILTIN_TYPE_TAGS


def _gen():
    return importlib.import_module("tools.gen_builtin_binding_guards")


def test_generated_outputs_are_byte_exact() -> None:
    gen = _gen()
    for path, expected in gen.render_all(gen.load_schema()).items():
        assert path.read_bytes() == expected.encode("utf-8"), (
            f"{path.relative_to(gen.ROOT)} is stale; run "
            "`python tools/gen_builtin_binding_guards.py --write`"
        )


def test_bits_are_dense_and_fit_an_inline_int() -> None:
    bits = list(BUILTIN_BINDING_GUARD_BITS.values())
    assert bits == [1 << index for index in range(len(bits))]
    assert BUILTIN_BINDING_GUARD_ALL_MASK == sum(bits)
    # The compiled mask is an inline int with a 47-bit signed payload.
    assert BUILTIN_BINDING_GUARD_ALL_MASK < 1 << 46


def test_rust_and_python_tables_assign_the_same_bits() -> None:
    gen = _gen()
    rust = gen.OUT_RUST.read_text(encoding="utf-8")
    arms = {
        name: 1 << int(bit)
        for name, bit in re.findall(r'b"([a-z_]+)" => 1 << (\d+)', rust)
    }
    assert arms == dict(BUILTIN_BINDING_GUARD_BITS)


def test_every_name_is_a_builtin_the_frontend_lowers() -> None:
    lowered = set(BUILTIN_FUNC_SPECS) | set(BUILTIN_TYPE_TAGS) | BUILTIN_SHAPE_NAMES
    for name in BUILTIN_BINDING_GUARD_BITS:
        assert hasattr(builtins, name), name
        assert name in lowered, name


@pytest.mark.parametrize(
    ("names", "message"),
    [
        (["len", "abs"], "sorted"),
        (["abs", "abs"], "repeats"),
        (["for"], "keyword"),
        ([f"n{index:02d}" for index in range(47)], "at most 46"),
    ],
)
def test_schema_rejects_inconsistent_tables(
    tmp_path, names: list[str], message: str
) -> None:
    source = tmp_path / "guards.toml"
    rows = ", ".join(f'"{name}"' for name in names)
    source.write_text(f"schema_version = 1\nnames = [{rows}]\n", encoding="utf-8")
    gen = _gen()
    with pytest.raises(gen.SchemaError, match=message):
        gen.load_schema(source)
