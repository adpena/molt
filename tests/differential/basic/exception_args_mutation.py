"""Purpose: differential coverage for exception args mutation."""

if __name__ == "__main__":
    try:
        raise ValueError("first")
    except ValueError as exc:
        print("before", exc.args)
        exc.args = ("second", 2)
        print("after", exc.args)
        print("str", str(exc))
