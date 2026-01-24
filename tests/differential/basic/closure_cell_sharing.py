"""Purpose: differential coverage for closure cell sharing semantics."""


def outer():
    x = []

    def add(value):
        x.append(value)
        return list(x)

    def snapshot():
        return list(x)

    return add, snapshot


if __name__ == "__main__":
    add, snapshot = outer()
    print("first", add(1))
    print("second", add(2))
    print("snap", snapshot())
