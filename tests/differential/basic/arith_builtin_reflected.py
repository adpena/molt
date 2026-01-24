"""Purpose: differential coverage for builtins using reflected ops."""

class Right:
    def __radd__(self, other):
        return f"radd:{other}"


if __name__ == "__main__":
    print("int_right", 3 + Right())
