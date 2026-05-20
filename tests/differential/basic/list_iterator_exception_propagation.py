"""Purpose: list/tuple/contains preserve non-terminal iterator exceptions."""


class BadNext:
    def __iter__(self):
        return self

    def __next__(self):
        raise TypeError("bad next")


class BadGetItem:
    def __getitem__(self, index):
        raise ValueError(f"bad get {index}")


class EmptyByIndex:
    def __getitem__(self, index):
        raise IndexError(index)


for ctor in (list, tuple):
    try:
        ctor(BadNext())
    except TypeError as exc:
        print(ctor.__name__, type(exc).__name__, str(exc))

try:
    list(BadGetItem())
except ValueError as exc:
    print("list-getitem", type(exc).__name__, str(exc))

try:
    1 in BadGetItem()
except ValueError as exc:
    print("contains-getitem", type(exc).__name__, str(exc))

print("index-empty-list", list(EmptyByIndex()))
print("index-empty-contains", 1 in EmptyByIndex())
