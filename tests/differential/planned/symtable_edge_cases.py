"""Purpose: differential coverage for symtable edge cases."""

import symtable

code = """
value = 1

def outer():
    x = 2
    def inner():
        return x
    return inner
"""

table = symtable.symtable(code, "<string>", "exec")
children = table.get_children()
print([child.get_name() for child in children])
print([child.get_type() for child in children])

outer = children[0]
inner = outer.get_children()[0]
print(outer.is_nested())
print(inner.is_nested())
print(inner.get_frees())
