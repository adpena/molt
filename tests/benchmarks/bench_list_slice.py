def main() -> None:
    data = list(range(10_000))

    total = 0
    for _ in range(1_000):
        chunk = data[100:9900:3]
        total += len(chunk)

    print(total)


if __name__ == "__main__":
    main()
