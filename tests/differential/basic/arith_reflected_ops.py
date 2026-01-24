"""Purpose: differential coverage for reflected arithmetic ops."""

class Left:
    def __mul__(self, other):
        return "left_mul"

    def __rmul__(self, other):
        return "left_rmul"


class Right:
    def __mul__(self, other):
        return NotImplemented

    def __rmul__(self, other):
        return "right_rmul"


if __name__ == "__main__":
    print("left_right", Left() * Right())
    print("right_left", Right() * Left())
