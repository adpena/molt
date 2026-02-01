def main() -> None:
    data: dict[int, int] = {}
    i = 0
    while i < 20_000:
        data[i] = i + 1
        i += 1

    total = 0
    i = 0
    while i < 20_000:
        total += data[i]
        i += 1

    print(total)


if __name__ == "__main__":
    main()
