"""Purpose: differential coverage for generator methods."""


class Counter:
    def __init__(self, n):
        self.n = n

    def __iter__(self):
        for i in range(self.n):
            yield i


def make_inner():
    class Inner:
        def __iter__(self):
            yield from [3, 4, 5]

    return Inner()


print(list(Counter(3)))
print(list(make_inner()))
