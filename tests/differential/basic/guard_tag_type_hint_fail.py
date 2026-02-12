"""Purpose: differential coverage for guard_tag type-hint checks."""


def add_one(x: int) -> int:
    return x + 1


print(add_one(4))
try:
    add_one("4")
except TypeError as exc:
    print(type(exc).__name__)
    print("hint-mismatch", "int" in str(exc))
