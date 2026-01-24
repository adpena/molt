"""Purpose: differential coverage for __length_hint__ usage in list()."""

class Seq:
    def __init__(self):
        self.count = 0

    def __iter__(self):
        return self

    def __next__(self):
        self.count += 1
        if self.count > 3:
            raise StopIteration
        return self.count

    def __length_hint__(self):
        return 3


if __name__ == "__main__":
    print("list", list(Seq()))
