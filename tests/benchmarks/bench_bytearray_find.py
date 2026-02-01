def main() -> None:
    data = bytearray()
    i = 0
    while i < 5_000:
        data.extend(b"ab")
        i += 1
    needle = bytearray(b"ab")

    total = 0
    i = 0
    while i < 10_000:
        total += data.find(needle)
        i += 1

    print(total)


if __name__ == "__main__":
    main()
