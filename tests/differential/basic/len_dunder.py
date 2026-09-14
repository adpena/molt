"""Purpose: differential coverage for len dunder."""


class HasLen:
    def __len__(self):
        return 3


class BoolLen:
    def __len__(self):
        return True


class NegLen:
    def __len__(self):
        return -1


class BigLen:
    def __len__(self):
        return 1 << 100


class BadLen:
    def __len__(self):
        return "nope"


class NoLen:
    pass


print(len(HasLen()))
print(len(BoolLen()))

try:
    len(NegLen())
except ValueError as exc:
    print(f"len-neg:{exc}")

try:
    len(BigLen())
except OverflowError as exc:
    print(f"len-big:{exc}")

try:
    len(BadLen())
except TypeError as exc:
    print(f"len-bad:{exc}")

try:
    len(NoLen())
except TypeError as exc:
    print(f"len-none:{exc}")


class StatefulLen:
    def __init__(self):
        self.calls = 0

    def __len__(self):
        self.calls += 1
        return self.calls


def repeated_callback_lengths(value):
    # A heap read with no intervening explicit CALL is still a Python callback.
    first = len(value)
    second = len(value)
    print("len-callbacks", first, second, value.calls)


repeated_callback_lengths(StatefulLen())


def mutable_length_after_alias_write():
    values = [1, 2]
    alias = values
    before = len(values)
    alias.append(3)
    after = len(values)
    immutable = (1, 2)
    print("len-mutation", before, after, len(immutable), values)


mutable_length_after_alias_write()
