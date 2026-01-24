"""Purpose: differential coverage for dict view set-like ops."""


def main():
    d1 = {"a": 1, "b": 2}
    d2 = {"b": 2, "c": 3}
    print("keys_and", d1.keys() & d2.keys())
    print("keys_or", d1.keys() | d2.keys())
    print("keys_sub", d1.keys() - d2.keys())


if __name__ == "__main__":
    main()
