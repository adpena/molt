def show(label, value):
    print(label, value)


def show_err(label, func):
    try:
        func()
    except Exception as exc:
        print(label, type(exc).__name__, exc)


b = b"  hello  world  "
show("find0", b.find(b"hello"))
show("find1", b.find(b"hello", 2))
show("find2", b.find(b"hello", 2, 6))
show("find3", b.find(b"", 3, 8))
show("find4", b.find(b"x", 0, 3))
show("find5", b.find(b"world", -10))
show("find6", b.find(b"world", -10, -1))
show("count0", b.count(b"l"))
show("count1", b.count(b"l", -8, -1))
show("starts0", b.startswith(b"hello", 2))
show("starts1", b.startswith((b"x", b"  he"), 0, 5))
show("starts2", b.startswith((), 0, 5))
show("ends0", b.endswith(b"  ", 0, len(b)))
show("ends1", b.endswith((b"x", b"ld  "), 0, len(b)))
show("ends2", b.endswith(b"world", -6, len(b)))

ba = bytearray(b"abcabc")
show("bafind0", ba.find(b"ab", 1))
show("bafind1", ba.find(b"", 2, 4))
show("bastarts0", ba.startswith((b"x", b"ab"), 0, 2))
show("baends0", ba.endswith(b"bc", 0, 3))
show("bacount0", ba.count(b"ab", -6, -1))

show_err("err_starts", lambda: b"abc".startswith((b"a", 1)))
show_err("err_find", lambda: b"abc".find("a"))
show_err("err_ba_find", lambda: bytearray(b"abc").find("a"))
