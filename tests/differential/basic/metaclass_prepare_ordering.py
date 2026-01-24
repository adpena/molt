"""Purpose: differential coverage for __prepare__ mapping ordering."""

class Meta(type):
    @classmethod
    def __prepare__(mcls, name, bases):
        return {"a": 1, "b": 2}


class Demo(metaclass=Meta):
    c = 3


if __name__ == "__main__":
    print("keys", list(Demo.__dict__.keys())[:4])
