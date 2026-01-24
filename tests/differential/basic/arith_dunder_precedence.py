"""Purpose: differential coverage for arithmetic dunder precedence."""

class Left:
    def __add__(self, other):
        return "left_add"

    def __radd__(self, other):
        return "left_radd"


class Right:
    def __add__(self, other):
        return NotImplemented

    def __radd__(self, other):
        return "right_radd"


if __name__ == "__main__":
    print("left_right", Left() + Right())
    print("right_left", Right() + Left())
