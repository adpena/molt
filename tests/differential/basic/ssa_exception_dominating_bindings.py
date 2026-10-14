"""Observable binding identity across callback exceptions, loops and resumes."""


def callback(stage, value, fail_at, observations):
    observations.append((stage, value))
    if stage == fail_at:
        raise ValueError(stage)


def observe_transfer(fail_at):
    observations = []
    value = 0
    try:
        value = 1
        callback("early", value, fail_at, observations)
        value = 2
        callback("late", value, fail_at, observations)
        value = 3
    except ValueError:
        observations.append(("caught", value))
    finally:
        observations.append(("finally", value))
    return value, observations


assert observe_transfer("early") == (
    1, [("early", 1), ("caught", 1), ("finally", 1)]
)
assert observe_transfer("late") == (
    2, [("early", 1), ("late", 2), ("caught", 2), ("finally", 2)]
)
assert observe_transfer(None) == (
    3, [("early", 1), ("late", 2), ("finally", 3)]
)
print("callback bindings", 1, 2, 3)


def repeated_definition():
    value = 10
    value = 11
    observations = []
    try:
        callback("same-block", value, "same-block", observations)
        value = 99
    except ValueError:
        return value, observations
    return value, observations


assert repeated_definition() == (11, [("same-block", 11)])
print("last binding before callback", 11)


def loop_bindings():
    observations = []
    total = 0
    for index in range(4):
        current = total + index
        try:
            callback(index, current, 2, observations)
            current += 10
        except ValueError:
            current += 100
        total = current
    return total, observations


assert loop_bindings() == (136, [(0, 0), (1, 11), (2, 23), (3, 126)])
print("loop bindings", 136)


def resumed_bindings(fail_at, observations):
    value = 5
    increment = yield ("start", value)
    value += increment
    try:
        callback("resume", value, fail_at, observations)
    except ValueError:
        yield ("caught", value)
    else:
        yield ("ok", value)
    value += 1
    yield ("end", value)


for fail_at, middle in [("resume", "caught"), (None, "ok")]:
    observations = []
    generator = resumed_bindings(fail_at, observations)
    assert next(generator) == ("start", 5)
    assert generator.send(7) == (middle, 12)
    assert observations == [("resume", 12)]
    assert next(generator) == ("end", 13)
    try:
        next(generator)
    except StopIteration:
        pass
    else:
        raise AssertionError("generator must finish")
    print("resume bindings", middle, 12, 13)
