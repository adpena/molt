"""Purpose: differential coverage for del on nonlocal bindings."""


def outer():
    x = "value"

    def inner():
        nonlocal x
        print("inner_before", x)
        del x
        try:
            print("inner_after", x)
        except Exception as exc:
            print("inner_after", type(exc).__name__)

    inner()
    try:
        print("outer_after", x)
    except Exception as exc:
        print("outer_after", type(exc).__name__)


if __name__ == "__main__":
    outer()
