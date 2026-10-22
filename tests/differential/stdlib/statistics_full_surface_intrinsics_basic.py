"""Purpose: differential coverage for intrinsic-lowered statistics surface."""

import builtins
import math
import statistics

data = [1.0, 2.0, 4.0, 7.0]
labels = ["a", "b", "a", "c", "b", "a"]

print("mean", statistics.mean(data))
print("fmean", statistics.fmean(data))
print("variance", round(statistics.variance(data), 6))
print("pvariance", round(statistics.pvariance(data), 6))
print("stdev", round(statistics.stdev(data), 6))
print("pstdev", round(statistics.pstdev(data), 6))
print("median", statistics.median(data))
print("median_low", statistics.median_low(data))
print("median_high", statistics.median_high(data))
print("median_grouped", round(statistics.median_grouped([10, 10, 20, 20, 20, 30]), 6))
print("mode", statistics.mode(labels))
print("multimode", statistics.multimode([1, 2, 2, 1, 3]))
print("quantiles_ex", [round(v, 6) for v in statistics.quantiles(data, n=4)])
print(
    "quantiles_in",
    [round(v, 6) for v in statistics.quantiles(data, n=4, method="inclusive")],
)
print("hmean", round(statistics.harmonic_mean([1.0, 2.0, 4.0]), 6))
print("gmean", round(statistics.geometric_mean([1.0, 2.0, 4.0]), 6))
print("cov", round(statistics.covariance([1.0, 2.0, 3.0], [2.0, 4.0, 6.0]), 6))
print("corr", round(statistics.correlation([1.0, 2.0, 3.0], [2.0, 4.0, 6.0]), 6))
print("linreg", statistics.linear_regression([1.0, 2.0, 3.0], [2.0, 4.0, 6.0]))
print(
    "linreg_prop",
    statistics.linear_regression([1.0, 2.0, 3.0], [2.0, 4.0, 6.0], proportional=True),
)

try:
    statistics.mode([])
except statistics.StatisticsError as exc:
    print(type(exc).__name__, str(exc))

try:
    statistics.quantiles([1.0], n=4)
except statistics.StatisticsError as exc:
    print(type(exc).__name__, str(exc))


# A callback ValueError must retain its identity rather than become the
# algorithm's StatisticsError. The empty-input controls take the owned branch.
class CallbackFailure(ValueError):
    pass


failure = CallbackFailure("statistics callback")


class FailedData:
    def __len__(self):
        return 2

    def __iter__(self):
        raise failure


unary_operations = (
    statistics.mean,
    statistics.fmean,
    statistics.variance,
    statistics.pvariance,
    statistics.stdev,
    statistics.pstdev,
    statistics.median,
    statistics.median_low,
    statistics.median_high,
    statistics.median_grouped,
    statistics.mode,
    statistics.quantiles,
    statistics.harmonic_mean,
    statistics.geometric_mean,
)
for operation in unary_operations:
    try:
        operation(FailedData())
    except Exception as exc:  # noqa: BLE001 -- report exact differential outcome
        print("iterator identity", operation.__name__, exc is failure)
    try:
        operation([])
    except Exception as exc:  # noqa: BLE001 -- report exact differential outcome
        print(
            "validation class",
            operation.__name__,
            type(exc) is statistics.StatisticsError,
        )

for operation in (
    statistics.covariance,
    statistics.correlation,
    statistics.linear_regression,
):
    try:
        operation(FailedData(), [1, 2])
    except Exception as exc:  # noqa: BLE001 -- report exact differential outcome
        print("paired iterator identity", operation.__name__, exc is failure)
    try:
        operation([], [])
    except Exception as exc:  # noqa: BLE001 -- report exact differential outcome
        print(
            "paired validation class",
            operation.__name__,
            type(exc) is statistics.StatisticsError,
        )


class FailedFloat:
    def __lt__(self, other):
        return False

    def __float__(self):
        raise failure


for operation in (
    lambda: statistics.fmean([FailedFloat()]),
    lambda: statistics.NormalDist(FailedFloat(), 1),
    lambda: statistics.NormalDist(0, FailedFloat()),
):
    try:
        operation()
    except Exception as exc:  # noqa: BLE001 -- report exact differential outcome
        print("conversion identity", exc is failure)

# The fused slice reductions bypass the Python wrappers and share the same
# validation owner. A successful slice still takes the vectorized algorithm.
values = [1.0, 2.0, 3.0]
print("slice success", statistics.mean(values[:]), statistics.stdev(values[:]))
for operation in (
    lambda: statistics.mean(values[:0]),
    lambda: statistics.stdev(values[:1]),
):
    try:
        operation()
    except Exception as exc:  # noqa: BLE001 -- report exact differential outcome
        print("slice validation class", type(exc) is statistics.StatisticsError)

print(
    "error metadata",
    statistics.StatisticsError.__module__,
    statistics.StatisticsError.__name__,
)
print("error base", statistics.StatisticsError.__bases__ == (ValueError,))
print("module error not builtin", hasattr(builtins, "StatisticsError"))


class DerivedStatisticsError(statistics.StatisticsError):
    def __init__(self, detail):
        super().__init__(detail, 7)
        self.detail = detail


error = DerivedStatisticsError("detail")
print("error subclass", error.args, error.detail, isinstance(error, ValueError))
try:
    math.sqrt(-1)
except Exception as exc:  # noqa: BLE001 -- report exact differential outcome
    print("math validation unchanged", type(exc) is ValueError)
