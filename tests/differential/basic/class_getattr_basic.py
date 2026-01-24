"""Purpose: differential coverage for class __getattr__."""

class Demo:
    value = 1

    @classmethod
    def __getattr__(cls, name):
        return f"missing:{name}"


if __name__ == "__main__":
    print("value", Demo.value)
    print("missing", Demo.unknown)
