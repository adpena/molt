"""Compiled-code symbol identity: one spelling, disjoint from the runtime's.

Oracle: the runtime ABI tables the generator emits (`RUNTIME_EXECUTION_ABIS`
and the intrinsic manifest) list every runtime symbol; a compiled symbol must
never equal one of them, and two distinct (module, local) pairs must never
share a symbol. The defect these catch: `molt.gpu.tensor.zeros` once compiled
to `molt_gpu_tensor__zeros`, the GPU intrinsic's own symbol.
"""

from __future__ import annotations

import ast

import pytest

from molt import compiled_symbols
from molt._intrinsic_symbols import INTRINSIC_SIGNATURES
from molt._wasm_abi_generated import RUNTIME_EXECUTION_ABIS
from molt.frontend import SimpleTIRGenerator
from tools.wasm_abi_gen.execution_context import _check_compiled_namespace_disjoint

_RUNTIME_SYMBOLS = frozenset(RUNTIME_EXECUTION_ABIS) | frozenset(
    symbol for symbol, _arity in INTRINSIC_SIGNATURES.values()
)


def test_spelling_is_injective_across_module_boundaries() -> None:
    spelled = {
        compiled_symbols.module_function_symbol("a.b", "c"),
        compiled_symbols.module_function_symbol("a_b", "c"),
        compiled_symbols.module_function_symbol("a", "b_c"),
        compiled_symbols.module_init_symbol("a.b.c"),
        compiled_symbols.module_init_symbol("a.b"),
        compiled_symbols.native_wrapper_symbol("a.b", "c"),
    }
    assert len(spelled) == 6
    # A local name never holds the module separator.
    with pytest.raises(ValueError):
        compiled_symbols.module_function_symbol("a", "b.c")


def test_generator_refuses_runtime_symbols_inside_the_compiled_namespace() -> None:
    _check_compiled_namespace_disjoint({"molt_ok", "__molt_ok", "fd_write"})
    for bad in ("py.json.dumps", "pyinit.json", "molt.gpu", "molt-gpu"):
        with pytest.raises(ValueError, match="C identifiers outside"):
            _check_compiled_namespace_disjoint({"molt_ok", bad})


def test_no_runtime_symbol_is_a_compiled_symbol() -> None:
    assert _RUNTIME_SYMBOLS
    assert not [s for s in _RUNTIME_SYMBOLS if compiled_symbols.is_compiled_symbol(s)]


def _lower_module(module_name: str, source: str) -> list[dict[str, object]]:
    generator = SimpleTIRGenerator(module_name=module_name)
    generator.visit(ast.parse(source))
    return generator.to_json(scope="module")["functions"]


def test_a_module_function_never_takes_its_intrinsic_symbol() -> None:
    functions = _lower_module(
        "molt.gpu.tensor",
        "def zeros(shape):\n    return shape\n\n"
        "def tensor_linear(a, b):\n    return (lambda: a)()\n",
    )
    names = {function["name"] for function in functions}
    assert "molt_gpu_tensor__zeros" in _RUNTIME_SYMBOLS
    assert names.isdisjoint(_RUNTIME_SYMBOLS)
    assert compiled_symbols.module_function_symbol("molt.gpu.tensor", "zeros") in names
    assert compiled_symbols.module_init_symbol("molt.gpu.tensor") in names
    # Every record names its owner; no consumer parses it out of the symbol.
    assert {function["module"] for function in functions} == {"molt.gpu.tensor"}
    assert all(compiled_symbols.is_compiled_symbol(name) for name in names)


def test_a_main_function_needs_no_reserved_rename() -> None:
    functions = _lower_module("app", "def main():\n    return 1\n")
    names = {function["name"] for function in functions}
    assert compiled_symbols.module_function_symbol("app", "main") in names


def test_serialization_refuses_a_claimed_local_without_a_body() -> None:
    generator = SimpleTIRGenerator(module_name="app")
    generator.visit(ast.parse("def f():\n    return 1\n\nf()\n"))
    symbol = compiled_symbols.module_function_symbol("app", "f")
    assert symbol in generator.funcs_map
    del generator.funcs_map[symbol]
    with pytest.raises(ValueError, match="local functions without bodies"):
        generator.to_json(scope="module")
