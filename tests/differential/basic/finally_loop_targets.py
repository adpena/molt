"""Lexical transfers must survive inlined cleanup and nested loop lowering."""


def override_return(continue_outer, values):
    seen = []
    for outer in values:
        try:
            for inner in [0]:
                return ("unreachable", inner)
        finally:
            seen.append(outer)
            if continue_outer:
                continue
            break
    else:
        seen.append("else")
    return seen


def override_break():
    seen = []
    for outer in range(3):
        try:
            break
        finally:
            seen.append(outer)
            continue
    else:
        seen.append("else")
    return seen


def override_continue():
    seen = []
    for outer in range(3):
        try:
            continue
        finally:
            seen.append(outer)
            break
    else:
        seen.append("wrong-else")
    return seen


def nested_while():
    seen = []
    for outer in range(3):
        inner = 0
        while inner < 2:
            inner += 1
            continue
        seen.append((outer, inner))
    return seen


def dynamic_range(step):
    seen = []
    for outer in range(0, 3 * step, step):
        try:
            for inner in [0]:
                return ("unreachable", inner)
        finally:
            seen.append(outer)
            continue
    return seen


def nested_finally():
    seen = []
    for outer in (0, 1):
        try:
            for inner in [0]:
                try:
                    return ("unreachable", inner)
                finally:
                    seen.append(("inner", outer))
        finally:
            try:
                seen.append(("outer", outer))
            finally:
                continue
    return seen


for values in ([0, 1, 2], (0, 1, 2), range(3)):
    result = override_return(False, values)
    assert result == [0], result
    print("break-return", result)
    result = override_return(True, values)
    assert result == [0, 1, 2, "else"], result
    print("continue-return", result)

result = override_break()
assert result == [0, 1, 2, "else"], result
print("continue-break", result)
result = override_continue()
assert result == [0], result
print("break-continue", result)
result = nested_while()
assert result == [(0, 2), (1, 2), (2, 2)], result
print("nested-while", result)
for step in (2, -2):
    result = dynamic_range(step)
    assert result == [0, step, 2 * step], result
    print("dynamic-range", step, result)
result = nested_finally()
assert result == [("inner", 0), ("outer", 0), ("inner", 1), ("outer", 1)], result
print("nested-finally", result)
