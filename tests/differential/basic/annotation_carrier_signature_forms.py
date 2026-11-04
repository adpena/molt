from typing import Optional, Union

Alias = Optional[float]
events = []

class Owner:
    def __init__(self, label):
        self.label = label
    def __del__(self):
        events.append(self.label)

def positional(x: float, /) -> float:
    return x
def keyword(*, x: bool) -> bool:
    return x
def star(*xs: float) -> float:
    return xs[0]
def starkw(**xs: bool) -> bool:
    return xs['x']
def union(x: float | None) -> float | None:
    return x
def optional(x: Alias) -> Alias:
    return x
def typing_union(x: Union[bool, None]) -> Union[bool, None]:
    return x

class Methods:
    def instance(self, x: float) -> float:
        return x
    @classmethod
    def class_method(cls, x: bool) -> bool:
        return x
    @staticmethod
    def static_method(x: int) -> int:
        return x

def exercise():
    methods = Methods()
    for kind in range(10):
        owner = Owner(kind)
        if kind == 0:
            result = positional(owner)
        elif kind == 1:
            result = keyword(x=owner)
        elif kind == 2:
            result = star(owner)
        elif kind == 3:
            result = starkw(x=owner)
        elif kind == 4:
            result = union(owner)
        elif kind == 5:
            result = optional(owner)
        elif kind == 6:
            result = typing_union(owner)
        elif kind == 7:
            result = methods.instance(owner)
        elif kind == 8:
            result = methods.class_method(owner)
        else:
            result = methods.static_method(owner)
        print(kind, result is owner, type(result).__name__)
        del owner
        print('held', events)
        del result
        print('released', events)
exercise()
