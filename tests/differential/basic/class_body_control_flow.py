"""Purpose: differential coverage for arbitrary control flow at class scope.

CPython executes a class body as a normal code block whose locals namespace is
the class dict.  Therefore for/if/while/AugAssign/try/with all "just work" and
bind into the class.  molt must match this exactly.
"""


class ForAug:
    x = 0
    for i in range(3):
        x += i  # for + AugAssign at class scope -> 0+1+2 = 3
    acc = []
    for j in range(4):
        acc.append(j * j)


class IfElse:
    y = 10
    if y > 5:
        z = 99  # taken branch binds z
    else:
        z = -1
    if y < 5:
        not_set = 1  # untaken branch must NOT bind
    flag = "big" if y > 5 else "small"


class WhileLoop:
    w = 0
    n = 0
    while n < 4:
        w += n
        n += 1  # 0+1+2+3 = 6, n ends at 4


class TryWith:
    seen = []

    try:
        seen.append("try")
        raise ValueError("boom")
    except ValueError as exc:
        seen.append("except:" + str(exc))
    else:
        seen.append("else")
    finally:
        seen.append("finally")


class _CM:
    def __enter__(self):
        return "resource"

    def __exit__(self, *a):
        return False


class WithBody:
    log = []
    with _CM() as r:
        log.append("with:" + r)


# Nested control flow + comprehension-adjacent binding
class Nested:
    total = 0
    for a in range(3):
        for b in range(3):
            if (a + b) % 2 == 0:
                total += 1


print("ForAug", ForAug.x, ForAug.acc)
print("IfElse", IfElse.z, getattr(IfElse, "not_set", "MISS"), IfElse.flag)
print("WhileLoop", WhileLoop.w, WhileLoop.n)
print("TryWith", TryWith.seen)
print("WithBody", WithBody.log)
print("Nested", Nested.total)
