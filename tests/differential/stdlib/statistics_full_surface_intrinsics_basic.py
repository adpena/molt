"""Purpose: differential coverage for intrinsic-lowered statistics surface."""

import builtins
import math
import statistics
from decimal import Decimal
from fractions import Fraction
from statistics import mean as imported_mean
from statistics import stdev as imported_stdev

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
# validation owner. Builtin reductions retain the guarded exact native lane.
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

# Exact reductions preserve the numeric domain, independently of whether the
# same data arrive as builtin storage, a one-shot iterator, or a slice.

numeric_cases = (
    ("int", [1, 2, 3]),
    ("float", [1.0, 2.0, 3.0]),
    ("mixed", [1, 2.0, 3]),
    ("bool", [True, True, True]),
    ("large", [2**60 + 1, 2**60 + 3]),
    ("cancel", [1e50, 1.0, -1e50]),
    ("zero", [-0.0, 0.0]),
    ("fraction", [Fraction(1, 3), Fraction(2, 3), Fraction(1)]),
    ("decimal", [Decimal("1.1"), Decimal("2.2"), Decimal("3.3")]),
)
for label, sample in numeric_cases:
    results = []
    for operation in (statistics.mean, statistics.variance, statistics.pvariance):
        direct = operation(sample)
        once = operation(iter(sample))
        results.append((type(direct).__name__, str(direct), direct == once))
    print("exact domain", label, results)
    sliced = imported_mean(sample[:])
    print("exact slice", label, type(sliced).__name__, str(sliced))

least = float.fromhex("0x0.0000000000001p-1022")
print("mean subnormal", statistics.mean([least, least]) == least)
print("mean half-even", statistics.mean([0.0, least]) == 0.0)
print("sqrt subnormal", statistics.pstdev([0.0, least * 2]) == least)
print("sqrt huge", statistics.pstdev([0, 2**1023]) == 2**1022)
print("center exact", statistics.variance([2**60 + 1, 2**60 + 3], 2**60 + 2))
print("sqrt decimal", statistics.stdev([Decimal(1), Decimal(2), Decimal(3)]))
print("sqrt fraction", statistics.stdev([Fraction(1), Fraction(2), Fraction(3)]))
print("quantile extrapolation", statistics.quantiles([1.0, 2.0], n=4))
print("quantile exact", statistics.quantiles([2**60 + 1, 2**60 + 3], n=4))
print("quantile decimal", statistics.quantiles([Decimal(1), Decimal(2)], n=4))
print("harmonic fraction", statistics.harmonic_mean([Fraction(1), Fraction(2)]))
print("harmonic decimal", statistics.harmonic_mean([Decimal(1), Decimal(2)]))
print("harmonic zero order", statistics.harmonic_mean([0, -1]))


class SelectedInt(int):
    pass


selected = SelectedInt(2)
print("median retains object", statistics.median([3, selected, 1]) is selected)
print("harmonic retains object", statistics.harmonic_mean([selected]) is selected)
print("mean subclass", type(statistics.mean([SelectedInt(1), selected])).__name__)

ratio_events = []


class RatioFloat(float):
    def as_integer_ratio(self):
        ratio_events.append("ratio")
        return super().as_integer_ratio()


print(
    "mean ratio subclass",
    type(statistics.mean([RatioFloat(1), RatioFloat(3)])).__name__,
)
print("ratio called once", ratio_events)


class FailedRatio:
    def as_integer_ratio(self):
        raise failure


for operation in (statistics.mean, statistics.variance, statistics.pstdev):
    try:
        operation([1.0, FailedRatio()])
    except Exception as exc:  # noqa: BLE001 -- independent callback identity oracle
        print("ratio failure identity", operation.__name__, exc is failure)


class MutatingBound:
    def __index__(self):
        indexed[:] = [5.0, 7.0, 9.0]
        return 0


indexed = [1.0, 2.0, 3.0]
print("slice index mutation", imported_mean(indexed[MutatingBound() :]))
print("slice huge bounds", imported_mean(indexed[-(2**100) : 2**100]))
print("slice stdev exact", imported_stdev(indexed[:]))
try:
    statistics.mean([Decimal(1), 2.0])
except TypeError:
    print("incompatible domains", True)


# Ordinary binding mutation must retain the callable loaded at the call site.
def rebound_statistics_call():
    from statistics import mean

    mean = lambda data: ("local callable", len(data))  # noqa: E731, F811 -- binding mutation is the oracle
    return mean(values[:])


print("slice retained callable", rebound_statistics_call())


# Local, unaliased imports exercised the retired slice-elision path.
def local_statistics_slices(data):
    from statistics import mean, stdev

    return mean(data[:]), stdev(data[:])


def local_statistics_bound(data, bound):
    from statistics import mean

    return mean(data[bound:])


print("local slices", local_statistics_slices([1.0, 2.0, 3.0]))
indexed = [1.0, 2.0, 3.0]
print("local slice mutation", local_statistics_bound(indexed, MutatingBound()))
print("local slice huge bound", local_statistics_bound(indexed, -(2**100)))
