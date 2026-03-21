def fibonacci(n):
    a, b = 0, 1
    for _ in range(n):
        a, b = b, a + b
    return a

def is_prime(n):
    if n < 2: return False
    if n < 4: return True
    if n % 2 == 0 or n % 3 == 0: return False
    i = 5
    while i * i <= n:
        if n % i == 0 or n % (i + 2) == 0: return False
        i += 6
    return True

def count_primes(limit):
    return sum(1 for n in range(2, limit + 1) if is_prime(n))

def diamond(size=9):
    lines = []
    for i in range(size):
        d = i if i < size // 2 + 1 else size - 1 - i
        lines.append(" " * (size // 2 - d) + "*" * (2 * d + 1))
    return "\n".join(lines)

def sort_data(data_str):
    nums = [int(x.strip()) for x in data_str.split(",") if x.strip()]
    nums.sort()
    return ", ".join(str(n) for n in nums)

def fizzbuzz(n):
    lines = []
    for i in range(1, n + 1):
        if i % 15 == 0: lines.append("FizzBuzz")
        elif i % 3 == 0: lines.append("Fizz")
        elif i % 5 == 0: lines.append("Buzz")
        else: lines.append(str(i))
    return "\n".join(lines)

import sys
path = sys.argv[1] if len(sys.argv) > 1 else "/"
query = sys.argv[2] if len(sys.argv) > 2 else ""

# Parse query params
params = {}
if query:
    for part in query.split("&"):
        if "=" in part:
            k, v = part.split("=", 1)
            params[k] = v

parts = path.strip("/").split("/")
route = parts[0] if parts else ""

if route == "fib":
    n = int(parts[1]) if len(parts) > 1 else 10
    print("fib(" + str(n) + ") = " + str(fibonacci(n)))

elif route == "primes":
    limit = int(parts[1]) if len(parts) > 1 else 100
    print("Primes up to " + str(limit) + ": " + str(count_primes(limit)))

elif route == "diamond":
    n = int(parts[1]) if len(parts) > 1 else 9
    print(diamond(n))

elif route == "mandelbrot":
    print(diamond(15))

elif route == "sort":
    data = params.get("data", "5,3,8,1,9,2,7,4,6")
    print("Sorted: " + sort_data(data))

elif route == "fizzbuzz":
    n = int(parts[1]) if len(parts) > 1 else 30
    print(fizzbuzz(n))

elif route == "pi":
    # Leibniz series for pi
    n = int(parts[1]) if len(parts) > 1 else 10000
    total = 0.0
    for i in range(n):
        total += ((-1.0) ** i) / (2.0 * i + 1.0)
    print("pi ≈ " + str(total * 4.0) + " (" + str(n) + " terms)")

else:
    print("Molt Python on Cloudflare Workers")
    print("==================================")
    print("")
    print("Compiled Python -> WASM, running at the edge.")
    print("Sub-millisecond execution. 2.8 MB binary.")
    print("")
    print("Try these endpoints with curl:")
    print("")
    print("  curl .../fib/50          Fibonacci numbers")
    print("  curl .../primes/10000    Count primes")
    print("  curl .../diamond/11      ASCII diamond pattern")
    print("  curl .../sort?data=5,3,1 Sort numbers")
    print("  curl .../fizzbuzz/100    FizzBuzz")
    print("  curl .../pi/1000000      Compute pi (Leibniz)")
    print("")
    print("Runtime: molt (compiled Python -> WASM)")
    print("Platform: Cloudflare Workers (free tier)")
