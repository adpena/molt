"""Purpose: differential coverage for dict view dynamic behavior."""


def main():
    d = {"a": 1, "b": 2}
    keys = d.keys()
    items = d.items()
    print("initial", list(keys), list(items))
    d["c"] = 3
    print("after_add", list(keys), list(items))
    d.pop("a")
    print("after_pop", list(keys), list(items))


if __name__ == "__main__":
    main()
