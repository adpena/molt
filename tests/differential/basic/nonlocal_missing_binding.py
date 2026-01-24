"""Purpose: differential coverage for nonlocal missing binding errors."""


def main():
    source = """

def outer():
    def inner():
        nonlocal x
        return x
    return inner
"""
    try:
        compile(source, "<nonlocal-missing>", "exec")
        print("missing", "missed")
    except SyntaxError as exc:
        print("missing", type(exc).__name__, exc.msg)


if __name__ == "__main__":
    main()
