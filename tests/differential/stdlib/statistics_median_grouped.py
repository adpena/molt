"""Purpose: grouped median and statistics numeric callback protocol coverage."""

import random
import statistics
import sys


def main():
    data = [1, 2, 2, 3, 4]
    print("median_grouped", statistics.median_grouped(data))


if __name__ == "__main__":
    main()


# Exercise the actual scalar bridge in the statistics child, using numeric
# protocols shared by Python float() and PyFloat_AsDouble. Text construction,
# float-subclass overrides, and user arithmetic are separate contracts.
events = []


class FloatInput:
    def __init__(self, label, value, failure=None):
        self.label = label
        self.value = value
        self.failure = failure

    def __float__(self):
        events.append(("float", self.label))
        if self.failure is not None:
            raise self.failure
        return self.value


class IndexInput:
    def __index__(self):
        events.append(("index", "interval"))
        return 2


for interval, expected_events in [
    (FloatInput("interval", 2.0), [("float", "interval")]),
    (IndexInput(), [("index", "interval")]),
]:
    events.clear()
    result = statistics.median_grouped([1, 2, 2, 3, 4], interval)
    assert result == 2.5
    assert events == expected_events
    print("interval_protocol", result, events)


events.clear()
dist = statistics.NormalDist(FloatInput("mu", 2.0), 3.0)
assert dist.mean == 2.0 and dist.stdev == 3.0
assert events == [("float", "mu")]
print("normal_mu_protocol", dist.mean, dist.stdev, events)

failure = RuntimeError("statistics numeric callback")
for label, operation in [
    (
        "interval",
        lambda: statistics.median_grouped(
            [1, 2, 2, 3, 4], FloatInput("interval", 2.0, failure)
        ),
    ),
    ("mu", lambda: statistics.NormalDist(FloatInput("mu", 2.0, failure), 3.0)),
]:
    events.clear()
    try:
        operation()
    except RuntimeError as caught:
        assert caught is failure
        assert events == [("float", label)]
        print("conversion_failure_identity", label, caught is failure, events)
    else:
        raise AssertionError("numeric callback failure was lost")


# Both supported sampling implementations propagate the real callback failure.
callback_name = "random" if sys.version_info >= (3, 14) else "gauss"
original_callback = getattr(random, callback_name)


def failed_sample(*args):
    events.append(("sample", len(args)))
    raise failure


try:
    setattr(random, callback_name, failed_sample)
    events.clear()
    try:
        dist.samples(2)
    except RuntimeError as caught:
        assert caught is failure
        assert len(events) == 1
        print("sample_failure_identity", caught is failure, events)
    else:
        raise AssertionError("sample callback failure was lost")
finally:
    setattr(random, callback_name, original_callback)
