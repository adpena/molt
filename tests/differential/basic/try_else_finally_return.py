"""Purpose: differential coverage for try/else/finally returns."""


def f(flag):
    try:
        if flag:
            return "try"
    except Exception:
        return "except"
    else:
        return "else"
    finally:
        pass


if __name__ == "__main__":
    print("true", f(True))
    print("false", f(False))
