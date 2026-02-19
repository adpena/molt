"""Purpose: differential coverage for codeop basic API surface."""

import codeop

ANNOTATIONS_FLAG = 0x1000000

print(codeop.PyCF_DONT_IMPLY_DEDENT == 0x200)
print(codeop.PyCF_ALLOW_INCOMPLETE_INPUT == 0x4000)

print(codeop.compile_command("x = 1") is not None)
print(codeop.compile_command("if True:\n    x = 1\n") is not None)
print(codeop.compile_command("if True:\n") is None)
print(codeop.compile_command("(", symbol="eval") is None)

try:
    codeop.compile_command("x =")
except SyntaxError as exc:
    print(type(exc).__name__, "incomplete input" in str(exc))

compiler = codeop.CommandCompiler()
print(compiler("from __future__ import annotations\n") is not None)
print(bool(compiler.compiler.flags & ANNOTATIONS_FLAG))
