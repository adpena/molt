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

n = 0
while n < 0:
    n += 1
else:
    print("while_else_empty", n)

n = 0
while n < 3:
    n += 1
    if n == 1:
        continue
else:
    print("while_else_continue", n)
