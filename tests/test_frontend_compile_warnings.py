from __future__ import annotations

import ast
import warnings

import pytest

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
        "~True\nwhile True:\n    try:\n        break\n    finally:\n        continue\n"
    )
    module = ast.parse(source)
    gen = SimpleTIRGenerator()

    gen._prescan_compile_warnings(module)

    assert gen._deferred_runtime_warnings == [
        "<string>:1: DeprecationWarning: Bitwise inversion '~' on bool is deprecated and will be removed in Python 3.16. This returns the bitwise inversion of the underlying int object and is usually not what you expect from negating a bool. Use the 'not' operator for boolean negation or ~int(x) if you really want the bitwise inversion of the underlying int.",
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


# Source-grounded CPython 3.14 ast_preprocess transfer boundaries. These source
# programs and warning sites were independently checked with compile(), rather
# than inferred from Molt's visitor state. Targets 3.12 and 3.13 emit none.
_FINALLY_WARNING_CASES = (
    ("def f():\n    try: pass\n    finally: return 1\n", [(3, "return")]),
    ("while True:\n    try: pass\n    finally: break\n", [(3, "break")]),
    ("for x in range(1):\n    try: pass\n    finally: continue\n", [(3, "continue")]),
    ("try: pass\nfinally:\n    for x in range(1):\n        break\n", []),
    ("try: pass\nfinally:\n    for x in range(1):\n        continue\n", []),
    ("try: pass\nfinally:\n    def f(): return 1\n", []),
    (
        "try: pass\nfinally:\n    def f():\n        try: pass\n        finally: return 1\n",
        [(5, "return")],
    ),
    (
        "for x in range(1):\n    try: pass\n    finally:\n        for y in []: pass\n        else: break\n",
        [(5, "break")],
    ),
    (
        "def f():\n    try: pass\n    except* Exception: pass\n    finally: return 1\n",
        [(4, "return")],
    ),
    (
        "def f():\n    if False:\n        try: pass\n        finally: return 1\n",
        [(4, "return")],
    ),
    (
        "def f():\n    try: pass\n    finally:\n        for x in range(1):\n            return 1\n",
        [],
    ),
    (
        "def f():\n    try: pass\n    finally: return 1; return 2\n",
        [(3, "return"), (3, "return")],
    ),
)


@pytest.mark.parametrize("target_python", [(3, 12), (3, 13), (3, 14)])
@pytest.mark.parametrize("source,sites", _FINALLY_WARNING_CASES)
def test_finally_warning_target_and_transfer_scope(source, sites, target_python):
    tree = ast.parse(source)
    generator = SimpleTIRGenerator(target_python=target_python)
    with warnings.catch_warnings(record=True) as observed:
        warnings.simplefilter("always", SyntaxWarning)
        generator._emit_finally_transfer_warnings(tree)
    expected = sites if target_python >= (3, 14) else []
    assert [
        (item.category, item.filename, item.lineno, str(item.message))
        for item in observed
    ] == [
        (SyntaxWarning, "<string>", line, f"'{transfer}' in a 'finally' block")
        for line, transfer in expected
    ]
    assert generator._deferred_runtime_warnings == []


def test_finally_warning_is_compiler_diagnostic_without_guest_output():
    source = "def f():\n    try: pass\n    finally: return 1\nf()\n"
    with warnings.catch_warnings(record=True) as observed:
        warnings.simplefilter("always", SyntaxWarning)
        generator = SimpleTIRGenerator(target_python=(3, 14))
        generator.visit(ast.parse(source))
        ir = generator.to_json()
    assert [(item.category, item.lineno) for item in observed] == [(SyntaxWarning, 3)]
    assert ir["functions"]
    assert not any(
        op["kind"] == "warn_stderr"
        for function in ir["functions"]
        for op in function["ops"]
    )


def test_finally_warning_filters_and_compiler_error_location(tmp_path):
    source = "def f():\n    try: pass\n    finally: return 1\n"
    path = tmp_path / "warning.py"
    path.write_text(source, encoding="utf-8")
    tree = ast.parse(source, filename=str(path))
    with warnings.catch_warnings(record=True) as observed:
        warnings.simplefilter("ignore", SyntaxWarning)
        SimpleTIRGenerator(target_python=(3, 14))._emit_finally_transfer_warnings(tree)
    assert observed == []
    with warnings.catch_warnings():
        warnings.simplefilter("error", SyntaxWarning)
        generator = SimpleTIRGenerator(target_python=(3, 14), source_path=str(path))
        with pytest.raises(SyntaxError, match="'return' in a 'finally' block") as error:
            generator._emit_finally_transfer_warnings(tree)
    assert error.value.filename == str(path)
    assert error.value.lineno == error.value.end_lineno == 3
    assert (error.value.offset, error.value.end_offset) == (14, 22)
    assert error.value.text == "    finally: return 1\n"


def test_support_pruning_keeps_bool_phase_separate_from_source_diagnostics():
    source = (
        "def kept(): return 1\n"
        "def discarded():\n"
        "    try: pass\n"
        "    finally: return ~True\n"
    )
    generator = SimpleTIRGenerator(
        target_python=(3, 14),
        module_name="support",
        known_modules={"support"},
        native_support_function_roots={"kept"},
    )
    with warnings.catch_warnings(record=True) as observed:
        warnings.simplefilter("always", SyntaxWarning)
        generator.visit(ast.parse(source))
        ir = generator.to_json()
    assert [(item.category, item.lineno) for item in observed] == [(SyntaxWarning, 4)]
    assert any(function["name"].endswith("kept") for function in ir["functions"])
    assert all("discarded" not in function["name"] for function in ir["functions"])
    assert not any(
        op["kind"] == "warn_stderr"
        for function in ir["functions"]
        for op in function["ops"]
    )


@pytest.mark.parametrize("exists", [False, True])
def test_finally_syntax_error_ignores_poisoned_linecache(tmp_path, monkeypatch, exists):
    import linecache

    path = tmp_path / "encoded.py"
    source = (
        "# coding: latin-1\ndef f():\n    try: pass\n    finally: return 1  # café\n"
    )
    if exists:
        path.write_bytes(source.encode("latin-1"))
    monkeypatch.setitem(
        linecache.cache,
        str(path),
        (100, None, ["poisoned\n"] * 4, str(path)),
    )
    generator = SimpleTIRGenerator(target_python=(3, 14), source_path=str(path))
    with warnings.catch_warnings():
        warnings.simplefilter("error", SyntaxWarning)
        with pytest.raises(SyntaxError) as error:
            generator._emit_finally_transfer_warnings(ast.parse(source))
    assert error.value.text == ("    finally: return 1  # café\n" if exists else None)
