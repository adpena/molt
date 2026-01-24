"""Purpose: differential coverage for walrus assignment scoping."""

x = 0
vals = [(x := i) for i in range(3)]
print("listcomp", x, vals)

x = 0
vals = [x := i for i in range(2, 5)]
print("listcomp_simple", x, vals)

x = 0
vals = list((x := i) for i in range(3))
print("genexpr", x, vals)

if (y := 5) > 0:
    print("if", y)

z = 0
while (z := z + 1) < 3:
    pass
print("while", z)
