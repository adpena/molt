"""GPU vector addition example using Molt's GPU compute support."""

from molt import gpu
import time

@gpu.kernel
def vector_add(a: gpu.Buffer[float], b: gpu.Buffer[float],
               c: gpu.Buffer[float], n: int):
    tid = gpu.thread_id()
    if tid < n:
        c[tid] = a[tid] + b[tid]


def main():
    n = 1024

    # Create input data
    a_host = [float(i) for i in range(n)]
    b_host = [float(i * 2) for i in range(n)]

    # Transfer to GPU
    a_gpu = gpu.to_device(a_host)
    b_gpu = gpu.to_device(b_host)
    c_gpu = gpu.alloc(n, float)

    # Launch kernel
    threads_per_block = 256
    num_blocks = (n + threads_per_block - 1) // threads_per_block
    total_threads = num_blocks * threads_per_block

    start = time.perf_counter()
    vector_add[total_threads](a_gpu, b_gpu, c_gpu, n)
    elapsed = time.perf_counter() - start

    # Read back results
    result = gpu.from_device(c_gpu)

    # Verify
    expected = [a + b for a, b in zip(a_host, b_host)]
    errors = sum(1 for r, e in zip(result, expected) if abs(r - e) > 1e-10)

    print(f"Vector add: {n} elements")
    print(f"Time: {elapsed:.6f}s")
    print(f"Errors: {errors}/{n}")
    if errors == 0:
        print("PASS")
    else:
        print("FAIL")
        for i in range(min(5, errors)):
            if abs(result[i] - expected[i]) > 1e-10:
                print(f"  [{i}]: got {result[i]}, expected {expected[i]}")


if __name__ == "__main__":
    main()
