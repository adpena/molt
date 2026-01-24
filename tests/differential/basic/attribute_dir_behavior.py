"""Purpose: differential coverage for __dir__ customization."""

class Demo:
    def __dir__(self):
        return ["a", "b"]


if __name__ == "__main__":
    demo = Demo()
    print("dir", dir(demo))
