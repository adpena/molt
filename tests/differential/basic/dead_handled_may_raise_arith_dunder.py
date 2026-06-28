class RaisesOnArithmetic:
    def __add__(self, other):
        raise ValueError("boom-add")

    def __sub__(self, other):
        raise ValueError("boom-sub")

    def __mul__(self, other):
        raise ValueError("boom-mul")

    def __eq__(self, other):
        raise ValueError("boom-eq")

    def __lt__(self, other):
        raise ValueError("boom-lt")

    def __neg__(self):
        raise ValueError("boom-neg")

    def __pos__(self):
        raise ValueError("boom-pos")

    def __abs__(self):
        raise ValueError("boom-abs")


def show_exc(label, thunk):
    try:
        result = thunk()
    except BaseException as exc:
        print(label, type(exc).__name__, "|", str(exc))
        return
    print(label, "DID-NOT-RAISE", repr(result))


def main():
    obj = RaisesOnArithmetic()

    def dead_add():
        unused = obj + 1
        return unused

    def dead_sub():
        unused = obj - 1
        return unused

    def dead_mul():
        unused = obj * 2
        return unused

    def dead_eq():
        unused = obj == 1
        return unused

    def dead_lt():
        unused = obj < 1
        return unused

    def dead_neg():
        unused = -obj
        return unused

    def dead_pos():
        unused = +obj
        return unused

    def dead_abs():
        unused = abs(obj)
        return unused

    show_exc("dead:add", dead_add)
    show_exc("dead:sub", dead_sub)
    show_exc("dead:mul", dead_mul)
    show_exc("dead:eq", dead_eq)
    show_exc("dead:lt", dead_lt)
    show_exc("dead:neg", dead_neg)
    show_exc("dead:pos", dead_pos)
    show_exc("dead:abs", dead_abs)

    try:
        _ = obj + 1
    except ValueError as exc:
        print("handled:add", str(exc))

    try:
        _ = obj * 3
    except ValueError as exc:
        print("handled:mul", str(exc))

    try:
        _ = abs(obj)
    except ValueError as exc:
        print("handled:abs", str(exc))

    for _ in range(0):
        unused = obj + 1
        print(unused)
    print("zero_trip", "no-spurious-raise")

    print("safe:add", 7 + 3)
    print("safe:sub", 10 - 4)
    print("safe:mul", 6 * 7)
    print("safe:eq", 7 == 3)
    print("safe:lt", 5 < 9)
    print("safe:neg", -5)
    print("safe:bitand", 6 & 3)
    print("safe:bitor", 4 | 1)


if __name__ == "__main__":
    main()
