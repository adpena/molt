def outer() -> int:
    x = 3

    def inner() -> int:
        return x + 1

    return inner()


def outer_lambda() -> int:
    y = 5
    return (lambda z: y + z)(7)


def outer_gen() -> list[int]:
    base = 10

    def gen():
        for i in range(2):
            yield base + i

    return list(gen())


print(outer())
print(outer_lambda())
print(outer_gen())
