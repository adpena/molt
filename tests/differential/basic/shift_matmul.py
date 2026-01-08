def main() -> None:
    print(1 << 2)
    print(8 >> 1)
    print(True << 3)
    print(False >> 2)
    print(-1 >> 100)
    try:
        _ = 1 << -1
    except ValueError:
        print("ValueError")
    try:
        _ = 1.5 << 2
    except TypeError:
        print("TypeError")
    try:
        _ = 1 >> "2"
    except TypeError:
        print("TypeError")
    try:
        _ = 1 @ 2
    except TypeError:
        print("TypeError")


if __name__ == "__main__":
    main()
