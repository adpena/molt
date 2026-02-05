"""Purpose: keyword argument coverage for builtins and list.index."""

values = [0, 1, 2, 1, 3]
print(values.index(1, 1))
print(values.index(1, 2, 4))
print(values.index(1, 1, 4))

try:
    values.index(1, foo=2)
    print("list-index-kw-ok")
except TypeError:
    print("list-index-kw-typeerror")

print(int("10", base=2))

try:
    int(x=5)
    print("int-x-ok")
except TypeError:
    print("int-x-typeerror")

try:
    int(base=10)
    print("int-base-ok")
except TypeError:
    print("int-base-typeerror")

print(round(number=1.25))
print(round(1.25, ndigits=1))

try:
    round(ndigits=2)
    print("round-missing-ok")
except TypeError:
    print("round-missing-typeerror")

try:
    round(1, foo=2)
    print("round-foo-ok")
except TypeError:
    print("round-foo-typeerror")

try:
    print(list(range(start=1, stop=5, step=2)))
    print("range-kw-ok")
except TypeError:
    print("range-kw-typeerror")
