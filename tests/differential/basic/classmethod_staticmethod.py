"""Purpose: differential coverage for @classmethod and @staticmethod builtins."""


class Counter:
    count = 0

    @classmethod
    def increment(cls):
        cls.count += 1
        return cls.count

    @classmethod
    def from_value(cls, n):
        obj = cls()
        cls.count = n
        return obj

    @staticmethod
    def validate(x):
        return isinstance(x, int) and x >= 0

    @staticmethod
    def add(a, b):
        return a + b


if __name__ == "__main__":
    # classmethod basic
    print("inc1", Counter.increment())
    print("inc2", Counter.increment())
    print("count", Counter.count)

    # classmethod receives cls
    obj = Counter.from_value(10)
    print("from_value count", Counter.count)

    # staticmethod basic
    print("validate 5", Counter.validate(5))
    print("validate -1", Counter.validate(-1))
    print("validate str", Counter.validate("x"))
    print("add", Counter.add(3, 4))

    # classmethod on instance
    c = Counter()
    print("instance classmethod", c.increment())

    # staticmethod on instance
    print("instance staticmethod", c.validate(7))

    # inheritance with classmethod
    class Sub(Counter):
        label = "sub"

    Sub.count = 0
    print("sub inc", Sub.increment())
    print("sub count", Sub.count)

    # classmethod receives subclass cls
    class Factory:
        @classmethod
        def create(cls):
            return cls.__name__

    class Child(Factory):
        pass

    print("factory parent", Factory.create())
    print("factory child", Child.create())

    # staticmethod does not receive cls/self
    class Util:
        @staticmethod
        def greet(name):
            return f"hello {name}"

    print("greet", Util.greet("world"))
    print("greet instance", Util().greet("test"))

    # classmethod with kwargs
    class Builder:
        @classmethod
        def build(cls, *, name, value=0):
            return f"{cls.__name__}:{name}={value}"

    print("builder", Builder.build(name="x", value=42))

    # staticmethod with default args
    class Math:
        @staticmethod
        def power(base, exp=2):
            return base ** exp

    print("power", Math.power(3))
    print("power3", Math.power(2, 10))

    # classmethod descriptor protocol
    print("classmethod type", type(Counter.__dict__["increment"]).__name__)
    print("staticmethod type", type(Counter.__dict__["validate"]).__name__)

    # callable checks
    print("classmethod callable", callable(Counter.increment))
    print("staticmethod callable", callable(Counter.validate))
