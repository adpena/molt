def main() -> None:
    values: list[int] = []
    i = 0
    while i < 10_000:
        values.append(i)
        i += 1
    data = tuple(values)

    total = 0
    for _ in range(1_000):
        chunk = data[100:9900:3]
        total += len(chunk)

    print(total)


if __name__ == "__main__":
    main()
