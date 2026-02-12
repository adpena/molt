"""Purpose: differential coverage for statistics.mean/statistics.stdev basics."""

import statistics

data = [1.0, 2.0, 4.0, 7.0]
print(round(statistics.mean(data), 6))
print(round(statistics.stdev(data), 6))
print(round(statistics.stdev(data, 3.5), 6))

try:
    statistics.mean([])
except statistics.StatisticsError as exc:
    print(type(exc).__name__, str(exc))

try:
    statistics.stdev([1.0])
except statistics.StatisticsError as exc:
    print(type(exc).__name__, str(exc))
