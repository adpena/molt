"""Purpose: differential coverage for nonlocal updates via comprehension calls."""


def outer():
    x = 0

    def bump():
        nonlocal x
        x += 1
        return x

    vals = [bump() for _ in range(3)]
    print("vals", vals)
    print("x", x)


outer()
