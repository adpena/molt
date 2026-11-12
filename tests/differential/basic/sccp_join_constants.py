# Constants across joins: a wrong conditional constant fold changes these results.


def equal_arms(flag):
    if flag:
        x = 5
    else:
        x = 5
    return x + 1


def unequal_arms(flag):
    if flag:
        x = 5
    else:
        x = 7
    return x + 1


def handler_value(divisor):
    value = 1
    try:
        value = 10 // divisor
        value = 2
    except ZeroDivisionError:
        value = 3
    return value * 100


def changing_loop(limit):
    i = 0
    total = 0
    while i < limit:
        total += i
        i += 1
    return i, total


def invariant_loop(limit):
    step = 4
    total = 0
    for _ in range(limit):
        total += step
        step = 4
    return step, total


def constant_predicate_pair():
    flag = False
    if flag:
        x = 1
    else:
        x = 2
    return x * x


def string_join(flag):
    if flag:
        s = "ab"
    else:
        s = "ab"
    return s + "c"


def none_join(flag):
    value = None if flag else None
    return value is None


print(equal_arms(True), equal_arms(False))
print(unequal_arms(True), unequal_arms(False))
print(handler_value(0), handler_value(5))
print(changing_loop(4), changing_loop(0))
print(invariant_loop(3), invariant_loop(0))
print(constant_predicate_pair())
print(string_join(True), string_join(False))
print(none_join(True), none_join(False))
