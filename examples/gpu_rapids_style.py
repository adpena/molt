"""RAPIDS-style GPU data processing example."""

from molt import gpu
from molt.gpu import ops
import time


def main():
    n = 10000

    # Create data
    data = ops.arange(0.0, float(n))
    print(f"Data: {n} elements")

    # Map: square each element
    start = time.perf_counter()
    squared = ops.map(lambda x: x * x, data)
    map_time = time.perf_counter() - start
    print(f"  map(x*x): {map_time:.4f}s")

    # Reduce: sum
    start = time.perf_counter()
    total = ops.reduce(squared, "sum")
    reduce_time = time.perf_counter() - start
    expected = sum(i * i for i in range(n))
    print(f"  reduce(sum): {total:.0f} (expected {expected:.0f}) — {reduce_time:.4f}s")

    # Filter: keep only values > threshold
    start = time.perf_counter()
    big = ops.filter(lambda x: x > n * n * 0.9, squared)
    filter_time = time.perf_counter() - start
    print(f"  filter(>90%): {big.size} elements — {filter_time:.4f}s")

    # Scan: cumulative sum
    small = ops.arange(1.0, 11.0)
    scanned = ops.scan(small, "sum")
    scan_result = gpu.from_device(scanned)
    print(f"  scan([1..10]): {scan_result}")

    # Dot product
    a = ops.linspace(0.0, 1.0, 100)
    b = ops.ones(100)
    d = ops.dot(a, b)
    print(f"  dot(linspace, ones): {d:.4f}")

    # Norm
    v = gpu.to_device([3.0, 4.0])
    print(f"  norm([3,4]): {ops.norm(v):.4f} (expected 5.0)")

    # Where (conditional select)
    cond = gpu.to_device([1, 0, 1, 0, 1])
    va = gpu.to_device([10.0, 20.0, 30.0, 40.0, 50.0])
    vb = gpu.to_device([0.0, 0.0, 0.0, 0.0, 0.0])
    selected = ops.where(cond, va, vb)
    print(f"  where: {gpu.from_device(selected)}")

    print("\nAll operations complete.")


if __name__ == "__main__":
    main()
