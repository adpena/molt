"""Purpose: differential coverage for metaclass __call__ return override."""

class Meta(type):
    def __call__(cls, *args, **kwargs):
        return {"cls": cls.__name__, "args": args}


class Demo(metaclass=Meta):
    def __init__(self, value):
        self.value = value


if __name__ == "__main__":
    print("result", Demo(3))
