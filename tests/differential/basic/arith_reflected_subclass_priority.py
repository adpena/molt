"""Purpose: differential coverage for reflected ops with subclass priority."""

class Base:
    def __add__(self, other):
        return "base_add"


class Sub(Base):
    def __radd__(self, other):
        return "sub_radd"


if __name__ == "__main__":
    print("base_sub", Base() + Sub())
    print("sub_base", Sub() + Base())
