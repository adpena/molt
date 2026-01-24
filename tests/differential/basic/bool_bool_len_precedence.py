"""Purpose: differential coverage for __bool__ vs __len__ precedence."""

class Both:
    def __bool__(self):
        return False

    def __len__(self):
        return 1


class OnlyLen:
    def __len__(self):
        return 2


if __name__ == "__main__":
    print("both", bool(Both()))
    print("only_len", bool(OnlyLen()))
