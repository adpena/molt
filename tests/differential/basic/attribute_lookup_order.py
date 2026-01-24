"""Purpose: differential coverage for attribute lookup order."""

class Parent:
    def __init__(self):
        self.instance = "instance"

    def __getattribute__(self, name):
        if name == "hook":
            return "getattribute"
        return super().__getattribute__(name)

    def __getattr__(self, name):
        return f"missing:{name}"

    @property
    def prop(self):
        return "property"


class Child(Parent):
    pass


if __name__ == "__main__":
    child = Child()
    print("instance", child.instance)
    print("prop", child.prop)
    print("hook", child.hook)
    print("missing", child.unknown)
