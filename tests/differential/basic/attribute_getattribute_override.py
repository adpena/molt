"""Purpose: differential coverage for __getattribute__ override priority."""

class Demo:
    value = "class"

    def __getattribute__(self, name):
        if name == "value":
            return "override"
        return super().__getattribute__(name)


if __name__ == "__main__":
    demo = Demo()
    print("value", demo.value)
