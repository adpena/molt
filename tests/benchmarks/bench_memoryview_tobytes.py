def main() -> None:
    data = bytearray()
    i = 0
    while i < 200:
        data.extend(b"0123456789abcdef")
        i += 1
    blob = bytes(data)
    view = memoryview(blob)
    view2 = view.cast("B", shape=[40, 80])
    view3 = view.cast("B", shape=[20, 10, 16])
    i = 0
    total = 0
    while i < 1000:
        total += view.tobytes()[0]
        total += view2.tobytes()[0]
        total += view3.tobytes()[0]
        i += 1

    print(total)


if __name__ == "__main__":
    main()
