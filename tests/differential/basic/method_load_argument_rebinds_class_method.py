"""Purpose: a method call loads its method before it evaluates the arguments (HF-216).

An argument that rebinds the method on the class, or deletes it, must not
change which function the call runs: CPython's LOAD_ATTR (method form) runs
first. The next call sees the new binding.
"""


class Counter:
    def bump(self, value):
        return ("old", value)


def rebind(value):
    Counter.bump = lambda self, value: ("new", value)
    return value


def delete(value):
    del Counter.bump
    return value


counter = Counter()
print(counter.bump(rebind(1)))
print(counter.bump(2))

Counter.bump = lambda self, value: ("restored", value)
print(counter.bump(delete(3)))
try:
    counter.bump(4)
except AttributeError as error:
    print("AttributeError", error)


class Base:
    def greet(self, name):
        return "base " + name


class Child(Base):
    def greet(self, name):
        return "child " + name


def drop_override(name):
    del Child.greet
    return name


child = Child()
print(child.greet(drop_override("a")))
print(child.greet("b"))


def loop():
    results = []
    for index in range(3):
        if index == 1:
            Base.greet = lambda self, name: "patched " + name
        results.append(child.greet(str(index)))
    return results


print(loop())
