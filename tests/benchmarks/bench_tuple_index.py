def main() -> None:
    data: list[int] = []
    i = 0
    while i < 1000:
        data.append(i)
        i += 1
    t = tuple(data)
    total = 0
    for _ in range(500):
        limit = len(t)
        i = 0
        while i < limit:
            total += t[i]
            i += 1

    print(total)


if __name__ == "__main__":
    main()
