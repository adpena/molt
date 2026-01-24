"""Purpose: differential coverage for nested comprehension capture of outer vars."""

vals = [[i + j for j in range(2)] for i in range(2)]
print("vals", vals)

funcs = [(lambda: i) for i in range(3)]
print("late", [f() for f in funcs])
