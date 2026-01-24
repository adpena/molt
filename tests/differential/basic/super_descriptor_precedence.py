"""Purpose: differential coverage for super descriptor precedence."""


class DataDesc:
    def __get__(self, obj, owner=None) -> str:
        name = getattr(owner, "__name__", "None")
        return f"data:{name}"

    def __set__(self, obj, val) -> None:
        obj._data = val


class NonDataDesc:
    def __get__(self, obj, owner=None) -> str:
        name = getattr(owner, "__name__", "None")
        return f"nondata:{name}"


class Base:
    data = DataDesc()
    nondata = NonDataDesc()


class Child(Base):
    def __init__(self) -> None:
        self.data = "inst-data"
        self.nondata = "inst-nondata"

    def probe(self) -> tuple[str, str, str, str, str]:
        return (self.data, self.nondata, super().data, super().nondata, self._data)


c = Child()
print(c.probe())
