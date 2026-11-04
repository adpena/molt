def integer_bool(flag):
    if flag:
        return True
    return 3
def float_int(flag):
    if flag:
        return 1.5
    return 3
def float_none(flag) -> float:
    if flag:
        return 1.5
    return None
def bool_none(flag) -> bool:
    if flag:
        return False
    return None
def wide_int(flag) -> int:
    if flag:
        return 1 << 60
    return 7
for f in (integer_bool, float_int, float_none, bool_none, wide_int):
    for flag in (True, False):
        value = f(flag)
        print(type(value).__name__, value)
