"""Purpose: differential coverage for deleting missing attributes."""

class Demo:
    pass


if __name__ == "__main__":
    demo = Demo()
    try:
        del demo.missing
    except AttributeError as exc:
        print("error", exc.name, exc.obj is demo)
