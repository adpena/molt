"""Purpose: differential coverage for f-string debug + format spec."""


value = 12.3456
width = 8
precision = 2

formatted = f"{value:{width}.{precision}f}"
debug = f"{value=}"
nested = f"{(lambda x: x + 1)(2)=}"

calls = []


def w() -> int:
    calls.append("w")
    return 6


def p() -> int:
    calls.append("p")
    return 1


order = f"{1:{w()}.{p()}f}"

print(formatted)
print(debug)
print(nested)
print(calls)
print(order)
