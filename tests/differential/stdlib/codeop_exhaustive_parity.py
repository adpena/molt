"""Purpose: CPython 3.12-derived exhaustive parity coverage for codeop."""

import codeop

ANNOTATIONS_FLAG = 0x1000000


def assert_valid(source, symbol="single"):
    out = codeop.compile_command(source, "<input>", symbol)
    assert out is not None, (symbol, source)


def assert_incomplete(source, symbol="single"):
    out = codeop.compile_command(source, "<input>", symbol)
    assert out is None, (symbol, source)


def assert_invalid(source, symbol="single"):
    try:
        codeop.compile_command(source, "<input>", symbol)
    except SyntaxError:
        return
    raise AssertionError((symbol, source))


# CPython 3.12 Lib/test/test_codeop.py::test_valid (selected broad surface)
assert codeop.compile_command("") is not None
assert codeop.compile_command("\n") is not None
assert_valid("a = 1")
assert_valid("\na = 1")
assert_valid("a = 1\n")
assert_valid("def x():\n  pass\n")
assert_valid("if 1:\n pass\n")
assert_valid("if 9==3:\n   pass\nelse:\n   pass\n")
assert_valid("#a\n#b\na = 3\n")
assert_valid("a = 9+ \\\n3")
assert_valid("3**3", "eval")
assert_valid("(lambda z: \n z**3)", "eval")
assert_valid("\n\na**3", "eval")
assert_valid("#a\n#b\na**3", "eval")
assert_valid("@a.b.c\ndef f():\n pass\n")

# CPython 3.12 Lib/test/test_codeop.py::test_incomplete (selected broad surface)
assert_incomplete("(a **")
assert_incomplete("a = (")
assert_incomplete("a = {")
assert_incomplete("if 1:")
assert_incomplete("if 1:\n")
assert_incomplete("def x():")
assert_incomplete("def x():\n")
assert_incomplete("def x():\n  pass")
assert_incomplete("a = 9+ \\")
assert_incomplete("a = 'a\\")
assert_incomplete("a = '''xy")
assert_incomplete("", "eval")
assert_incomplete("\n", "eval")
assert_incomplete("(", "eval")
assert_incomplete("(9+", "eval")
assert_incomplete("9+ \\", "eval")
assert_incomplete("lambda z: \\", "eval")
assert_incomplete("if a:\n pass\nelif b:")
assert_incomplete("while a:")
assert_incomplete("for a in b:")
assert_incomplete("try:")
assert_incomplete("with a:")
assert_incomplete("class a:")

# CPython 3.12 Lib/test/test_codeop.py::test_invalid + test_invalid_exec (selected)
assert_invalid("a b")
assert_invalid("a = ")
assert_invalid("a = 9 +")
assert_invalid("def x():\n\npass\n")
assert_invalid("a = 9+ \\\n")
assert_invalid("a = 1", "eval")
assert_invalid("9+", "eval")
assert_invalid("lambda z:", "eval")
assert_invalid("return 2.3")
assert_invalid("if (a == 1 and b = 2): pass")
assert_invalid("raise = 4", "exec")
assert_invalid("def a-b", "exec")
assert_invalid("await?", "exec")
assert_invalid("=!=", "exec")
assert_invalid("a await raise b", "exec")
assert_invalid("a await raise b?+1", "exec")
assert_invalid("del 1")
assert_invalid("del (1,)")
assert_invalid("del [1]")
assert_invalid("del '1'")
assert_invalid("[i for i in range(10)] = (1, 2, 3)")
assert_invalid("]", "eval")
assert_invalid("())", "eval")
assert_invalid("[}", "eval")
assert_invalid("a b", "eval")

# Additional incomplete coverage from CPython test_codeop.py.
assert_incomplete("if 9==3:\n   pass\nelse:\n   pass")
assert_incomplete("if a:\n pass\nelif b:\n pass\nelse:")
assert_incomplete("while a:\n pass\nelse:")
assert_incomplete("for a in b:\n pass\nelse:")
assert_incomplete("try:\n pass\nexcept:\n pass\nfinally:")
assert_incomplete("class a():")

# CPython 3.12 Lib/test/test_codeop.py::test_filename
assert codeop.compile_command("a = 1\n", "abc").co_filename == "abc"
assert codeop.compile_command("a = 1\n", "abc").co_filename != "def"

# CPython 3.12 Lib/test/test_codeop.py::test_syntax_errors
try:
    codeop.compile_command("def foo(x,x):\n   pass\n", symbol="exec")
except SyntaxError as exc:
    assert "duplicate argument 'x'" in str(exc)
else:
    raise AssertionError("expected duplicate-argument SyntaxError")

# Compile() remembers future flags.
compile_state = codeop.Compile()
assert bool(compile_state.flags & codeop.PyCF_DONT_IMPLY_DEDENT)
assert bool(compile_state.flags & codeop.PyCF_ALLOW_INCOMPLETE_INPUT)
compile_state("from __future__ import annotations\n", "<input>", "single")
assert bool(compile_state.flags & ANNOTATIONS_FLAG)

# CommandCompiler() delegates and remembers future flags.
command_state = codeop.CommandCompiler()
command_state("from __future__ import annotations\n")
assert bool(command_state.compiler.flags & ANNOTATIONS_FLAG)

# Module surface shape.
assert codeop.__all__ == ["compile_command", "Compile", "CommandCompiler"]

print("codeop_exhaustive_parity_ok")
