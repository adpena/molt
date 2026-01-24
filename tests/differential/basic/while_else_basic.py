"""Purpose: differential coverage for while-else semantics."""

n = 0
while n < 2:
    n += 1
else:
    print("while_else", n)

n = 0
while n < 2:
    n += 1
    if n == 1:
        break
else:
    print("while_else_break", n)
