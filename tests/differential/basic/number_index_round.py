"""Purpose: differential coverage for __index__ and __round__."""

class Index:
    def __index__(self):
        return 3


class Round:
    def __round__(self, ndigits=None):
        return ("round", ndigits)


if __name__ == "__main__":
    data = [0, 1, 2, 3, 4]
    print("index", data[Index()])
    print("round0", round(Round()))
    print("round2", round(Round(), 2))
