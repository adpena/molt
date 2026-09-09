"""Class LOAD_NAME/LOAD_CLASSDEREF retains its mapping after callbacks."""

value = "global"


class Loop:
    value = "class"
    for iteration in (1, 2):
        def method(self):
            return value

    observed = value
    del value
    fallback = value


print(Loop.observed, Loop.fallback)


class Comprehension:
    value = "class"
    inputs = (1,)
    observed = [value for iteration in inputs]


print(Comprehension.observed)


class Missing(KeyError):
    pass


class Namespace(dict):
    def __delitem__(self, name):
        raise ValueError(name)

    def __getitem__(self, name):
        if name == "injected":
            return "prepared"
        if name == "value":
            raise Missing(name)
        if name == "broken":
            raise ValueError("mapping-error")
        return super().__getitem__(name)


class Meta(type):
    @classmethod
    def __prepare__(mcls, name, bases):
        return Namespace()


class Prepared(metaclass=Meta):
    observed = injected
    fallback = value


print(Prepared.observed, Prepared.fallback)
try:
    class Broken(metaclass=Meta):
        observed = broken
except ValueError as error:
    print(str(error))

try:
    class Delete(metaclass=Meta):
        del absent
except NameError as error:
    print(str(error), error.__context__ is None)


def enclosing():
    captured = "outer"

    class Capture:
        for iteration in (1,):
            observed = captured

    class Rebind:
        nonlocal captured
        captured = "changed"
        observed = captured

    class Outer:
        captured = "not-lexical"

        class Inner:
            for iteration in (1,):
                observed = captured

    return Capture.observed, Rebind.observed, captured, Outer.Inner.observed


print(enclosing())
