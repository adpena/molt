"""Purpose: two runs of one class statement create distinct classes with the same layout, so each run's methods read and write the other run's instances correctly (HF-220)."""


def make(tag):
    class Box:
        def __init__(self, value):
            self.tag = tag
            self.value = value

        def read(self):
            return (self.tag, self.value)

        def write(self, value):
            self.value = value

    return Box


First = make("first")
Second = make("second")
old = First(1)
new = Second(2)
print(First is Second, type(old) is type(new))
print(Second.read(old))
print(First.read(new))
Second.write(old, 10)
First.write(new, 20)
print(old.read(), new.read())
print(type(old).__qualname__, type(new).__qualname__)
