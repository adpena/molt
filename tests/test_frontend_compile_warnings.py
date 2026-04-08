from __future__ import annotations

import ast

from molt.frontend import SimpleTIRGenerator


def test_prescan_compile_warnings_walks_module_once(
    monkeypatch,
) -> None:
    source = "\n".join(
        [
            "value = 1",
            "~True",
            "try:",
            "    value += 1",
            "finally:",
            "    return_value = 3",
        ]
        + [f"value += {i}" for i in range(200)]
    )
    module = ast.parse(source)
    gen = SimpleTIRGenerator()

    orig_walk = ast.walk
    module_walks = 0

    def counting_walk(node):
        nonlocal module_walks
        if node is module:
            module_walks += 1
        return orig_walk(node)

    monkeypatch.setattr(ast, "walk", counting_walk)

    gen._prescan_compile_warnings(module)

    assert module_walks <= 1


def test_prescan_compile_warnings_collects_expected_messages() -> None:
    source = (
        "~True\n"
        "while True:\n"
        "    try:\n"
        "        break\n"
        "    finally:\n"
        "        continue\n"
    )
    module = ast.parse(source)
    gen = SimpleTIRGenerator()

    gen._prescan_compile_warnings(module)

    assert gen._deferred_runtime_warnings == [
        "<string>:1: DeprecationWarning: Bitwise inversion '~' on bool is deprecated and will be removed in Python 3.16. This returns the bitwise inversion of the underlying int object and is usually not what you expect from negating a bool. Use the 'not' operator for boolean negation or ~int(x) if you really want the bitwise inversion of the underlying int.",
        "<string>:6: SyntaxWarning: 'continue' in a 'finally' block",
    ]


def test_prescan_compile_warnings_skips_nested_scopes_in_finally() -> None:
    source = (
        "try:\n"
        "    pass\n"
        "finally:\n"
        "    def inner():\n"
        "        return 1\n"
        "    class Local:\n"
        "        def method(self):\n"
        "            return 2\n"
    )
    module = ast.parse(source)
    gen = SimpleTIRGenerator()

    gen._prescan_compile_warnings(module)

    assert gen._deferred_runtime_warnings == []


def test_prescan_compile_warnings_state_is_isolated_per_generator() -> None:
    module = ast.parse("~True\n")

    gen_a = SimpleTIRGenerator()
    gen_a._prescan_compile_warnings(module)

    gen_b = SimpleTIRGenerator()
    gen_b._prescan_compile_warnings(module)

    assert gen_a._deferred_runtime_warnings is not gen_b._deferred_runtime_warnings
    assert gen_a._emitted_syntax_warnings is not gen_b._emitted_syntax_warnings
    assert gen_b._deferred_runtime_warnings == [
        "<string>:1: DeprecationWarning: Bitwise inversion '~' on bool is deprecated and will be removed in Python 3.16. This returns the bitwise inversion of the underlying int object and is usually not what you expect from negating a bool. Use the 'not' operator for boolean negation or ~int(x) if you really want the bitwise inversion of the underlying int.",
    ]
