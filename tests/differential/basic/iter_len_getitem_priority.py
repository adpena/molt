"""Purpose: differential coverage for __len__ + __getitem__ iteration."""

class Seq:
    def __len__(self):
        return 3

    def __getitem__(self, index):
        return index


if __name__ == "__main__":
    print("values", list(Seq()))
