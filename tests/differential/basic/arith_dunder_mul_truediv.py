"""Purpose: differential coverage for __mul__/__truediv__ fallback ordering."""

class Left:
    def __mul__(self, other):
        return NotImplemented

    def __rmul__(self, other):
        return "left_rmul"

    def __truediv__(self, other):
        return "left_div"


class Right:
    def __mul__(self, other):
        return "right_mul"

    def __rtruediv__(self, other):
        return "right_rdiv"


if __name__ == "__main__":
    print("mul", Left() * Right())
    print("div", Left() / Right())
