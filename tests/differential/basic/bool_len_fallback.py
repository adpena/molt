"""Purpose: differential coverage for bool() fallback to __len__."""

class HasLen:
    def __len__(self):
        return 2


class ZeroLen:
    def __len__(self):
        return 0


if __name__ == "__main__":
    print("has_len", bool(HasLen()))
    print("zero_len", bool(ZeroLen()))
