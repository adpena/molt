"""Purpose: differential coverage for __floordiv__/__mod__ fallback ordering."""

class Left:
    def __floordiv__(self, other):
        return NotImplemented

    def __rfloordiv__(self, other):
        return "left_rfloordiv"

    def __mod__(self, other):
        return "left_mod"


class Right:
    def __floordiv__(self, other):
        return "right_floordiv"

    def __rmod__(self, other):
        return "right_rmod"


if __name__ == "__main__":
    print("floordiv", Left() // Right())
    print("mod", Left() % Right())
