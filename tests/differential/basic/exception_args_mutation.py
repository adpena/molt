"""Purpose: differential coverage for exception args mutation."""

if __name__ == "__main__":
    try:
        raise ValueError("first")
    except ValueError as exc:
        print("before", exc.args)
        exc.args = ("second", 2)
        print("after", exc.args)
        print("str", str(exc))

    stop = StopIteration(1)
    print("stop-before", stop.args, stop.value)
    stop.args = (2,)
    print("stop-assign", stop.args, stop.value)
    stop.__init__(3)
    print("stop-init", stop.args, stop.value)
