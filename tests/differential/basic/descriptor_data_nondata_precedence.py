"""Purpose: differential coverage for data vs non-data descriptor precedence."""


def make_value(tag):
    def getter(_):
        return tag
    return property(getter)


class NonData:
    def __get__(self, obj, owner):
        return "nondatadesc"


class Data:
    def __get__(self, obj, owner):
        return "datadesc"

    def __set__(self, obj, value):
        obj.__dict__["data"] = value


class Base:
    nondata = NonData()
    data = Data()


if __name__ == "__main__":
    base = Base()
    base.__dict__["nondata"] = "instance"
    base.__dict__["data"] = "instance"
    print("nondata", base.nondata)
    print("data", base.data)
