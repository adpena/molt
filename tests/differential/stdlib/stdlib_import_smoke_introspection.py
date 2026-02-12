"""Purpose: stdlib import smoke for introspection helpers."""

import dis
import opcode
import symtable
import codeop

modules = [
    dis,
    opcode,
    symtable,
    codeop,
]
print([mod.__name__ for mod in modules])
