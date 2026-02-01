"""Purpose: differential coverage for symtable basic API surface."""

import symtable

code = """x = 1


def f(a):
    b = a + 1
    return b
"""

table = symtable.symtable(code, "<string>", "exec")
print(table.get_type())
print(sorted(table.get_identifiers()))

child = table.get_children()[0]
print(child.get_name())
print(child.get_type())
print(sorted(child.get_identifiers()))
