"""Purpose: differential coverage for exec in function locals."""


def inner():
    x = 1
    exec("x = 2")
    return x, locals().get("x")


if __name__ == "__main__":
    result = inner()
    print("result", result)
