"""Purpose: differential coverage for locals capture in closures."""


def outer():
    x = 1
    def inner():
        return locals().get("x"), x
    return inner


if __name__ == "__main__":
    fn = outer()
    print("result", fn())
