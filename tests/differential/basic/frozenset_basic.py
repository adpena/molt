def main() -> None:
    fs = frozenset([1, 2, 2, 3])
    print(len(fs))
    print(1 in fs, 4 in fs)
    total = 0
    for x in fs:
        total += x
    print(total)
    s = {3, 4}
    u = fs | s
    print(len(u))
    print(1 in u, 4 in u)
    i = fs & s
    print(len(i))
    print(3 in i, 2 in i)
    d = fs - s
    print(len(d))
    print(1 in d, 3 in d)
    x = fs ^ s
    print(len(x))
    print(1 in x, 3 in x, 4 in x)


if __name__ == "__main__":
    main()
