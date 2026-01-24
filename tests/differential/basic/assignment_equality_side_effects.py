"""Purpose: differential coverage for assignment vs equality side effects."""

log = []


class Token:
    def __init__(self, value):
        self.value = value

    def __eq__(self, other):
        log.append(("eq", self.value, getattr(other, "value", other)))
        if isinstance(other, Token):
            return self.value == other.value
        return False


if __name__ == "__main__":
    left = Token(1)
    right = Token(1)
    alias = left
    print("assigned", alias is left)
    print("log_after_assign", log)
    print("equals", left == right)
    print("log_after_eq", log)
