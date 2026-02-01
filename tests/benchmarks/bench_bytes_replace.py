def main() -> None:
    data = bytearray()
    i = 0
    while i < 1000:
        data.extend(b"abc")
        i += 1
    blob = bytes(data)
    i = 0
    total = 0
    while i < 1000:
        out = blob.replace(b"ab", b"ba")
        total += len(out)
        i += 1

    print(total)


if __name__ == "__main__":
    main()
