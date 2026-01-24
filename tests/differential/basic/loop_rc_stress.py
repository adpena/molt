"""Purpose: differential coverage for loop rc stress."""

total = 0
i = 0
while i < 5000:
    lst = [i, i + 1, i + 2]
    d = {"a": i, "b": i + 1}
    b = b"abc" + b"def"
    total = total + len(lst) + len(d) + len(b)
    i = i + 1
print(total)
