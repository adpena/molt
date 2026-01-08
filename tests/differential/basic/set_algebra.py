def main() -> None:
    a = {1, 2, 3}
    b = {3, 4}
    u = a | b
    print(len(u))
    print(1 in u, 4 in u, 5 in u)
    i = a & b
    print(len(i))
    print(3 in i, 2 in i)
    d = a - b
    print(len(d))
    print(1 in d, 3 in d)
    x = a ^ b
    print(len(x))
    print(1 in x, 3 in x, 4 in x)


if __name__ == "__main__":
    main()
