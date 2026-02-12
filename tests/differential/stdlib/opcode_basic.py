"""Purpose: differential coverage for opcode basic API surface."""

import opcode

load_const = opcode.opmap["LOAD_CONST"]
print(load_const)
print(opcode.opname[load_const])
print("RETURN_VALUE" in opcode.opmap)
