"""Purpose: differential coverage for sorted() kw-only key/reverse."""

if __name__ == "__main__":
    try:
        sorted([3, 2, 1], None, True)
        print("ok")
    except TypeError:
        print("TypeError")
