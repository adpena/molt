class DataDesc:
    def __get__(self, obj, objtype=None) -> str:
        return "data"

    def __set__(self, obj, val) -> None:
        obj._set = val


class NonDataDesc:
    def __get__(self, obj, objtype=None) -> str:
        if obj is None:
            return "nondata-class"
        return "nondata"


class Holder:
    data = DataDesc()
    nondata = NonDataDesc()

    def __init__(self) -> None:
        self._set = "unset"
        self.data = "instance-data"
        self.nondata = "instance-nondata"


h = Holder()
print(h._set)
print(h.data)
print(h.nondata)
print(Holder.nondata)
