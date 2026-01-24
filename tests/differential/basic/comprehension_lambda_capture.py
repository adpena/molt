"""Purpose: differential coverage for comprehension lambda capture semantics."""

funcs = [lambda i=i: i for i in range(3)]
print("vals", [f() for f in funcs])

funcs_bad = [lambda: i for i in range(3)]
print("late", [f() for f in funcs_bad])
