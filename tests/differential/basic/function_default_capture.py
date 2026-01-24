"""Purpose: differential coverage for default arg capture timing."""

if __name__ == "__main__":
    value = "first"

    def f(arg=value):
        return arg

    value = "second"
    print("result", f())
