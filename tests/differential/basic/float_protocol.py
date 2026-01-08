class Floaty:
    def __float__(self):
        return 1.25


class Indexy:
    def __index__(self):
        return 7


print(float(Floaty()), float(Indexy()))


class BadFloat:
    def __float__(self):
        return 1


try:
    float(BadFloat())
except Exception as e:
    print(e)


class BadIndex:
    def __index__(self):
        return 1.5


try:
    float(BadIndex())
except Exception as e:
    print(e)


print(pow(2, 5, 7))
print(pow(3, -1, 11))

try:
    pow(2.0, 3, 5)
except Exception as e:
    print(e)

try:
    pow(2, 3, 0)
except Exception as e:
    print(e)

try:
    pow(2, -1, 4)
except Exception as e:
    print(e)
