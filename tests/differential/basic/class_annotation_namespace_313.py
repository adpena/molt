"""Static 3.13+ class annotation nested scopes and type parameter defaults."""
# MOLT_META: min_py=3.13

x = "global"


def outer_lambda():
    x = "outer-lambda"

    class Capture:
        x = "class-lambda"
        type Alias = lambda: x

    return Capture


class Comprehension:
    x = "class-comprehension"
    items = (0,)
    type Alias = [x for item in items]


print("lambda fallback", outer_lambda().Alias.__value__())
print("comprehension fallback", Comprehension.Alias.__value__)


class Defaults:
    marker = int
    type Alias[T = marker] = T


Defaults.marker = float
print("default", Defaults.Alias.__type_params__[0].__default__ is float)
