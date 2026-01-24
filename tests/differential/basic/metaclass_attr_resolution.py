"""Purpose: differential coverage for metaclass attribute resolution."""

class Meta(type):
    attr = "meta"

    def __getattribute__(cls, name):
        if name == "hook":
            return "meta_hook"
        return super().__getattribute__(name)


class Demo(metaclass=Meta):
    attr = "class"


if __name__ == "__main__":
    print("attr", Demo.attr)
    print("hook", Demo.hook)
