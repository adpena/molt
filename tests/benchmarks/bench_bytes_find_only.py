def main() -> None:
    size = 10_000_000
    data = bytearray(size)
    i = 0
    while i < size:
        data[i] = 97
        i += 1
    data.append(98)
    haystack = bytes(data)
    needle = b"b"
    i = 0
    total = 0
    while i < 200:
        total = total + haystack.find(needle)
        i += 1
    print(total)


if __name__ == "__main__":
    main()
