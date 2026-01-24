"""Purpose: differential coverage for metaclass descriptor precedence."""

class Desc:
    def __get__(self, obj, owner):
        return "desc"


class Meta(type):
    attr = Desc()


class Demo(metaclass=Meta):
    attr = "class"


if __name__ == "__main__":
    print("attr", Demo.attr)
