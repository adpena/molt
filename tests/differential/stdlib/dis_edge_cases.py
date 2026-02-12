"""Purpose: differential coverage for dis edge cases."""

import dis


def f(x):
    if x:
        return 1
    return 2

ops = [instr.opname for instr in dis.Bytecode(f)]
print(ops[:3])
print(ops[-3:])
print("POP_JUMP_FORWARD_IF_FALSE" in ops or "POP_JUMP_IF_FALSE" in ops)
