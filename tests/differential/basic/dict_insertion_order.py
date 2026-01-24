"""Purpose: differential coverage for dict insertion order semantics."""


def main():
    d = {"a": 1, "b": 2, "c": 3}
    print("keys", list(d.keys()))

    d["b"] = 9
    print("update", list(d.keys()))

    d["d"] = 4
    print("append", list(d.keys()))

    d.pop("b")
    d["b"] = 5
    print("reinsert", list(d.keys()))


if __name__ == "__main__":
    main()
