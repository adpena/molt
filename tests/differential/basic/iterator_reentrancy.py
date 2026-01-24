"""Purpose: differential coverage for iterator reentrancy on __iter__."""

class SelfIter:
    def __init__(self):
        self.count = 0

    def __iter__(self):
        return self

    def __next__(self):
        self.count += 1
        if self.count > 2:
            raise StopIteration
        return self.count


if __name__ == "__main__":
    it = SelfIter()
    print("iter_is_self", iter(it) is it)
    print("vals", list(it))
