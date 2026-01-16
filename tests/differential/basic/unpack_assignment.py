from __future__ import annotations


def capture(label: str, func) -> None:
    try:
        func()
    except Exception as exc:  # noqa: BLE001
        print(label, type(exc).__name__, str(exc))


a, b = [1, 2]
print("list", a, b)

a, b = (3, 4)
print("tuple", a, b)

a, b = iter([5, 6])
print("iter", a, b)

pairs = [(1, 2), (3, 4)]
loop_out = []
for x, y in pairs:
    loop_out.append(x + y)
print("for", loop_out)

a, (b, c) = [1, (2, 3)]
print("nested", a, b, c)

a, *b, c = [1, 2, 3, 4, 5]
print("star_mid", a, b, c)

*a, b, c = [1, 2, 3]
print("star_start", a, b, c)

a, b, *c = [1, 2, 3, 4]
print("star_end", a, b, c)

a, *b, c = [1, 2]
print("star_exact", a, b, c)

(*a,) = [1, 2]
print("star_only", a)

pairs = [(1, 2, 3), (4, 5, 6)]
loop_star_out = []
for x, *rest in pairs:
    loop_star_out.append((x, rest))
print("for_star", loop_star_out)


def unpack_too_few() -> None:
    a, b = [1]
    print(a, b)


def unpack_too_many() -> None:
    a, b = [1, 2, 3]
    print(a, b)


def unpack_star_too_few() -> None:
    a, *b, c = [1]
    print(a, b, c)


capture("too_few", unpack_too_few)
capture("too_many", unpack_too_many)
capture("star_too_few", unpack_star_too_few)
