"""Purpose: differential coverage for builtin constructor dynamic."""


def main():
    f = list
    print(f([1, 2, 3]))
    t = tuple
    print(t([1, 2, 3]))
    d = dict
    print(d([("a", 1), ("b", 2)]))
    s = set
    print(sorted(s([3, 1, 2])))
    fs = frozenset
    print(sorted(fs([3, 1, 2])))
    r = range
    print(list(r(3)))
    sl = slice
    sl_obj = sl(1, 4, 2)
    print(sl_obj.start, sl_obj.stop, sl_obj.step)
    b = bytes
    print(b(b"hi"))
    ba = bytearray
    print(ba(b"hi"))


if __name__ == "__main__":
    main()
