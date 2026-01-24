"""Purpose: differential coverage for __ne__ override semantics."""


class WeirdTrue:
    def __eq__(self, other):
        return True

    def __ne__(self, other):
        return True


class WeirdFalse:
    def __eq__(self, other):
        return False

    def __ne__(self, other):
        return False


class NeNotImplemented:
    def __eq__(self, other):
        return getattr(other, "tag", None) == "same"

    def __ne__(self, other):
        return NotImplemented


class Tag:
    def __init__(self, tag):
        self.tag = tag


class Left:
    def __eq__(self, other):
        return True

    def __ne__(self, other):
        return NotImplemented


class Right:
    def __ne__(self, other):
        return True


a = WeirdTrue()
print("ne_true_eq_true", a != a)

b = WeirdFalse()
print("ne_false_eq_false", b != b)

c = NeNotImplemented()
print("ne_notimpl_same", c != Tag("same"))
print("ne_notimpl_diff", c != Tag("diff"))

print("ne_rhs", Left() != Right())
