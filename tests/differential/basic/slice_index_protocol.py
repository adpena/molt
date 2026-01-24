"""Purpose: differential coverage for slice indices using __index__."""

log = []


class Index:
    def __init__(self, value):
        self.value = value

    def __index__(self):
        log.append(self.value)
        return self.value


if __name__ == "__main__":
    data = [0, 1, 2, 3, 4, 5]
    print("slice", data[Index(1):Index(5):Index(2)])
    print("log", log)
