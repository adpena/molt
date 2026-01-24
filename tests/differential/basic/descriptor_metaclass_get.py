"""Purpose: differential coverage for descriptor access on class vs metaclass."""

class Desc:
    def __get__(self, obj, owner):
        return (obj is None, owner.__name__)


class Meta(type):
    meta = Desc()


class Demo(metaclass=Meta):
    attr = Desc()


if __name__ == "__main__":
    print("class_attr", Demo.attr)
    print("meta_attr", Demo.meta)
