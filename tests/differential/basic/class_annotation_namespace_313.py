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


class ParameterDefaults[T]:
    type Alias[U = T] = U
    type Bound[U: T] = U
    type Constraints[U: (T, int)] = U


parameter = ParameterDefaults.__type_params__[0]
print("parameter default", ParameterDefaults.Alias.__type_params__[0].__default__ is parameter)
print("parameter bound", ParameterDefaults.Bound.__type_params__[0].__bound__ is parameter)
print("parameter constraints", ParameterDefaults.Constraints.__type_params__[0].__constraints__ == (parameter, int))
print("parameter alias", ParameterDefaults.Alias.__value__ is ParameterDefaults.Alias.__type_params__[0])


class ForwardDefault[T = U, U = int]:
    pass


print("forward default", ForwardDefault.__type_params__[0].__default__ is ForwardDefault.__type_params__[1])
