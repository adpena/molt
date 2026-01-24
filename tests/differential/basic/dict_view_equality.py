"""Purpose: differential coverage for dict view equality semantics."""


def main():
    d = {"a": 1, "b": 2}
    print("keys_eq", d.keys() == {"a", "b"})
    print("items_eq", d.items() == {("a", 1), ("b", 2)})


if __name__ == "__main__":
    main()
