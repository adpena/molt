"""Purpose: differential coverage for codeop basic API surface."""

import codeop

compiler = codeop.CommandCompiler()
print(compiler("x = 1
") is None)
print(compiler("def f():
    return 1
") is None)
print(compiler("if True:
    x = 1
") is None)
