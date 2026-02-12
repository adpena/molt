"""Purpose: differential coverage for exec locals/global scope semantics."""


def main():
    ns = {}
    exec("x = 1\ny = x + 1", ns)
    print("ns", ns.get("x"), ns.get("y"))

    scope = {"x": 2}
    exec("x = x + 3\n", scope)
    print("scope", scope["x"])


if __name__ == "__main__":
    main()
