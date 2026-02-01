"""Purpose: differential coverage for opcode table shapes."""

import opcode

print(len(opcode.opname) >= len(opcode.opmap))
print("LOAD_CONST" in opcode.opmap)
print("RETURN_VALUE" in opcode.opmap)
print("RESUME" in opcode.opmap)
