class Mapping:
    def __init__(self):
        self.data = {"a": 1, "b": 2}

    def keys(self):
        return ["a", "b"]

    def __getitem__(self, key):
        return self.data[key]


def f(**kw):
    return kw["a"], kw["b"], list(kw.items())


print(f(**Mapping()))

try:
    f(**42)
except TypeError:
    print("typeerror")

try:
    f(a=1, **{"a": 2})
except TypeError:
    print("dupe")


class BadKeys:
    def keys(self):
        return [1]

    def __getitem__(self, key):
        return "x"


try:
    f(**BadKeys())
except TypeError:
    print("keytype")
