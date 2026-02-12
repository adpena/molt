"""Purpose: differential coverage for statistics variance/pvariance/pstdev basics."""

import statistics

data = [1.0, 2.0, 4.0, 7.0]
print(round(statistics.variance(data), 6))
print(round(statistics.variance(data, 3.5), 6))
print(round(statistics.pvariance(data), 6))
print(round(statistics.pvariance(data, 3.5), 6))
print(round(statistics.pstdev(data), 6))
print(round(statistics.pstdev(data, 3.5), 6))

try:
    statistics.variance([1.0])
except statistics.StatisticsError as exc:
    print(type(exc).__name__, str(exc))

try:
    statistics.pvariance([])
except statistics.StatisticsError as exc:
    print(type(exc).__name__, str(exc))

try:
    statistics.pstdev([])
except statistics.StatisticsError as exc:
    print(type(exc).__name__, str(exc))
