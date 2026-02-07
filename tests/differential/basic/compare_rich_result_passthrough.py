"""Purpose: rich-compare operators must preserve non-bool return values."""


class Ret:
    def __init__(self, label, truth):
        self.label = label
        self.truth = truth

    def __bool__(self):
        print("bool", self.label)
        return self.truth

    def __repr__(self):
        return f"Ret({self.label})"


class Ord:
    def __init__(self, value):
        self.value = value

    def __lt__(self, other):
        return Ret(f"lt:{self.value}:{other.value}", self.value < other.value)

    def __le__(self, other):
        return Ret(f"le:{self.value}:{other.value}", self.value <= other.value)


class EqOnly:
    def __init__(self, value):
        self.value = value

    def __eq__(self, other):
        return ("eq", self.value, other.value)


class NeOnly:
    def __init__(self, value):
        self.value = value

    def __eq__(self, other):
        return ("eq-ne", self.value, other.value)

    def __ne__(self, other):
        return ("ne", self.value, other.value)


a = Ord(1)
b = Ord(2)
c = Ord(3)

print("single_lt", a < b)
print("single_le", a <= b)
print("chain_true", a < b <= c)
print("chain_false", c < b <= a)

x = EqOnly(1)
y = EqOnly(2)
print("eq_value", x == y)
print("ne_from_eq", x != y)

n1 = NeOnly(1)
n2 = NeOnly(2)
print("ne_value", n1 != n2)
