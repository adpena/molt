"""Purpose: differential coverage for nested comprehensions with nonlocal defs."""


def outer():
    x = 0

    def inner():
        nonlocal x
        x += 1
        return x

    vals = [inner() for _ in range(3)]
    return vals, x


print("vals", outer())
