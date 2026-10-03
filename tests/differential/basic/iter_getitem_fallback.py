"""Purpose: differential coverage for iter fallback to __getitem__."""

class Seq:
    def __init__(self):
        self.values = ["a", "b", "c"]

    def __getitem__(self, index):
        return self.values[index]


if __name__ == "__main__":
    print("values", list(Seq()))


class DerivedIndex(IndexError):
    pass


class IndexError(Exception):
    pass


class Exhaustion:
    def __init__(self, error):
        self.error = error

    def __getitem__(self, index):
        if index == 0:
            return "first"
        raise self.error


derived = DerivedIndex("finished")
derived.__class__.__name__ = "RenamedIndex"
print("subclass-stop", "missing" in Exhaustion(derived), list(Exhaustion(derived)))
impostor = IndexError("different identity")
for operation in (lambda obj: "missing" in obj, lambda obj: list(obj)):
    try:
        operation(Exhaustion(impostor))
    except Exception as caught:
        print("impostor-propagates", caught is impostor)
    else:
        print("impostor-propagates", False)
