"""Purpose: differential coverage for iter fallback to __getitem__."""

class Seq:
    def __init__(self):
        self.values = ["a", "b", "c"]

    def __getitem__(self, index):
        return self.values[index]


if __name__ == "__main__":
    print("values", list(Seq()))
