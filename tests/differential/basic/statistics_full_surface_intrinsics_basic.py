"""Purpose: differential coverage for intrinsic-lowered statistics surface."""

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
