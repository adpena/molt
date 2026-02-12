"""Purpose: differential coverage for dis basic API surface."""

import dis


def f(x):
    return x + 1

print([instr.opname for instr in dis.Bytecode(f)])
