def main() -> None:
    size = 10_000_000
    data = bytearray(size)
    i = 0
    while i < size:
        data[i] = 97
        i += 1
    data.append(98)
    haystack = bytes(data)
    print(haystack.find(b"b"))


if __name__ == "__main__":
    main()
