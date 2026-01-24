"""Purpose: differential coverage for list contains eq."""


class Boom:
    def __eq__(self, other):
        raise RuntimeError("boom")


values = [Boom()]
try:
    1 in values
except RuntimeError:
    print("contains-raises")
else:
    print("contains-missed")

items = [1, 2, 3]
print(items.__getitem__(1))
try:
    items.__getitem__("x")
except TypeError:
    print("getitem-typeerror")
else:
    print("getitem-noerror")
