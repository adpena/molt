"""Purpose: differential coverage for nested lambdas capturing comprehension vars."""

funcs = [
    (lambda: (lambda: i)())
    for i in range(3)
]
print("nested", [f() for f in funcs])
