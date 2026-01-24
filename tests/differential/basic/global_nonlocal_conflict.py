"""Purpose: differential coverage for global/nonlocal conflicts."""


def main():
    source = """

def outer():
    x = 1
    def inner():
        global x
        nonlocal x
        return x
    return inner
"""
    try:
        compile(source, "<global-nonlocal>", "exec")
        print("conflict", "missed")
    except SyntaxError as exc:
        print("conflict", type(exc).__name__, exc.msg)


if __name__ == "__main__":
    main()
