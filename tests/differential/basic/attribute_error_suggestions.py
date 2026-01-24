"""Purpose: differential coverage for AttributeError name/suggestions."""

class Demo:
    def __init__(self):
        self.value = 1


if __name__ == "__main__":
    demo = Demo()
    try:
        demo.vale
    except AttributeError as exc:
        print("name", exc.name)
        print("obj", exc.obj is demo)
