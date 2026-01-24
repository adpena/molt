"""Purpose: differential coverage for datamodel dunder operations."""

class Box:
    def __init__(self, value):
        self.value = value

    def __repr__(self):
        return f"Box({self.value})"

    def __str__(self):
        return f"Box[{self.value}]"

    def __bool__(self):
        return self.value != 0

    def __len__(self):
        return abs(self.value)

    def __contains__(self, item):
        return item == self.value


if __name__ == "__main__":
    box = Box(2)
    print("repr", repr(box))
    print("str", str(box))
    print("bool", bool(box))
    print("len", len(box))
    print("contains", 2 in box)
