"""Purpose: differential coverage for assignment aliasing and mutability."""


def main():
    a = [1, 2]
    b = a
    b.append(3)
    print("alias", a, b)

    c = (1, 2)
    d = c
    print("tuple_alias", c is d)

    a = [1, 2]
    b = a[:]
    b.append(3)
    print("copy", a, b)


if __name__ == "__main__":
    main()
