"""Purpose: differential coverage for walrus compare."""


def get(value):
    print(f"get={value}")
    return value


if (x := get(2)) > 1:
    print(f"x={x}")

print((y := None) is None)
print(f"y={y}")
