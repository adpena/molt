"""Purpose: differential coverage for __getitem__ raising IndexError termination."""

class Seq:
    def __getitem__(self, index):
        if index >= 2:
            raise IndexError
        return index * 2


if __name__ == "__main__":
    print("values", list(Seq()))
