"""Purpose: differential coverage for operator getters."""

import operator

obj = {"a": 1, "b": 2}
get_a_b = operator.itemgetter("a", "b")
print(get_a_b(obj))

class C:
    def __init__(self):
        self.x = {"y": 5}

c = C()
print(operator.attrgetter("x.y")(c))
print(operator.methodcaller("count", "a")("banana"))
