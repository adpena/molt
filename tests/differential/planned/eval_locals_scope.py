"""Purpose: differential coverage for eval locals/globals scope."""


def main():
    ns = {"x": 5}
    print("eval", eval("x + 1", ns))

    locals_ns = {"x": 3}
    globals_ns = {"x": 10}
    print("locals", eval("x + 2", globals_ns, locals_ns))


if __name__ == "__main__":
    main()
