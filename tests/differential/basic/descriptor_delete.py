class DeleteDesc:
    def __get__(self, obj, objtype=None) -> str:
        return "value"

    def __delete__(self, obj) -> None:
        obj.deleted = "yes"


class WithDesc:
    d = DeleteDesc()

    def __init__(self) -> None:
        self.deleted = "no"


w = WithDesc()
del w.d
print(w.deleted)


def get_x(self) -> str:
    return self._x


def del_x(self) -> None:
    self._x = "gone"


class WithProp:
    def __init__(self) -> None:
        self._x = "alive"

    x = property(get_x, None, del_x)


p = WithProp()
del p.x
print(p._x)

p2 = WithProp()
name = "x"
delattr(p2, name)
print(p2._x)

q = WithProp()
q.extra = "present"
del q.extra
print(hasattr(q, "extra"))
