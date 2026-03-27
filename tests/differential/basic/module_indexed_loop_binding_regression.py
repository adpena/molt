"""Purpose: native indexed loops must preserve module-scope bindings and in-loop stores."""

holder = [None]
for _name in ("hello",):
    holder[0] = _name

print(holder[0])
print(_name)
