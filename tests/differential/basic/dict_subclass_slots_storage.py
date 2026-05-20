"""Purpose: dict-subclass storage does not overwrite declared slots."""


class SlotDict(dict):
    __slots__ = ("handle",)

    def __init__(self):
        self.handle = 7

    def after_contains(self, key):
        print("before", self.handle)
        print("contains", key in self)
        print("after", self.handle)

    def after_get(self, key):
        print("before-get", self.handle)
        print("get", dict.get(self, key, "missing"))
        print("after-get", self.handle)


d = SlotDict()
d.after_contains("x")
d.after_get("x")
d["x"] = 11
print("item", d["x"], "slot", d.handle)
