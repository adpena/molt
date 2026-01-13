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
