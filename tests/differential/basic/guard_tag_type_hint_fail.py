"""Purpose: differential coverage for guard_tag type-hint checks."""


def add_one(x: int) -> int:
    return x + 1


print(add_one(4))
try:
    add_one("4")
except TypeError as exc:
    print(type(exc).__name__)
    print("hint-mismatch", "int" in str(exc))



def annotated_branch(flag):
    z = 5
    if flag:
        total: float = 0
        z = 7
    if z == 5:
        return "five"
    return "seven"


def annotated_loop(limit):
    i = 0
    total = 0
    while i < limit:
        marker: float = i
        total += i
        i += 1
    return i, total


print("annotated-branch", annotated_branch(False), annotated_branch(True))
print("annotated-loop", annotated_loop(4))
