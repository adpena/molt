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

def primes_up_to(limit):
    return [n for n in range(2, limit + 1) if is_prime(n)]

def diamond(size=9):
    lines = []
    for i in range(size):
        d = i if i < size // 2 + 1 else size - 1 - i
        lines.append(" " * (size // 2 - d) + "*" * (2 * d + 1))
    return "\n".join(lines)

def mandelbrot_render(width: int = 100, height: int = 40) -> str:
    mx: float = -2.0
    dx: float = 3.0 / width
    my: float = -1.1
    dy: float = 2.2 / height
    out: list = []
    row: int = 0
    while row < height:
        y0: float = my + dy * row
        col: int = 0
        line: str = ""
        while col < width:
            x0: float = mx + dx * col
            x: float = 0.0
            y: float = 0.0
            it: int = 0
            while it < 30:
                xx: float = x * x
                yy: float = y * y
                if xx + yy > 4.0:
                    break
                y = 2.0 * x * y + y0
                x = xx - yy + x0
                it = it + 1
            if it < 2:
                line = line + " "
            elif it < 4:
                line = line + "."
            elif it < 8:
                line = line + "+"
            elif it < 16:
                line = line + "*"
            elif it < 30:
                line = line + "#"
            else:
                line = line + "@"
            col = col + 1
        out.append(line)
        row = row + 1
    return "\n".join(out)

def sort_data(data_str):
    nums = [int(x.strip()) for x in data_str.split(",") if x.strip()]
    return nums

def fizzbuzz(n):
    lines = []
    for i in range(1, n + 1):
        if i % 15 == 0: lines.append("FizzBuzz")
        elif i % 3 == 0: lines.append("Fizz")
        elif i % 5 == 0: lines.append("Buzz")
        else: lines.append(str(i))
    return "\n".join(lines)

def safe_int(s, default, lo=0, hi=1000000):
    """Parse int from string with bounds. Returns default on bad input."""
    try:
        v = int(s)
    except (ValueError, TypeError):
        return default
    if v < lo:
        return lo
    if v > hi:
        return hi
    return v

def fmt_big(n):
    """Format a large number with commas."""
    s = str(n)
    if len(s) <= 3:
        return s
    parts = []
    while len(s) > 3:
        parts.append(s[-3:])
        s = s[:-3]
    parts.append(s)
    parts.reverse()
    return ",".join(parts)

def truncate_num(s, digits=50):
    """Truncate a number string representation."""
    if len(s) <= digits:
        return s
    return s[:digits] + "... (" + fmt_big(len(s)) + " digits)"

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
    n = safe_int(parts[1] if len(parts) > 1 else "", 100, 0, 10000)
    result = fibonacci(n)
    result_str = str(result)
    print("Fibonacci")
    print("=" * 40)
    print("")
    print("  n      = " + str(n))
    print("  fib(n) = " + truncate_num(result_str))
    if len(result_str) > 1:
        print("  digits = " + fmt_big(len(result_str)))
    print("")
    print("Compiled Python on Cloudflare Workers")

elif route == "primes":
    limit = safe_int(parts[1] if len(parts) > 1 else "", 10000, 2, 100000)
    found = primes_up_to(limit)
    count = len(found)
    print("Prime Numbers")
    print("=" * 40)
    print("")
    print("  range  = 2 to " + fmt_big(limit))
    print("  count  = " + fmt_big(count))
    print("")
    if count <= 20:
        primes_strs = [str(p) for p in found]
        print("  primes = " + ", ".join(primes_strs))
    else:
        first5_strs = [str(p) for p in found[:5]]
        last5_strs = [str(p) for p in found[-5:]]
        first5 = ", ".join(first5_strs)
        last5 = ", ".join(last5_strs)
        print("  first  = " + first5)
        print("  last   = " + last5)
    print("")
    print("Compiled Python on Cloudflare Workers")

elif route == "diamond":
    n = safe_int(parts[1] if len(parts) > 1 else "", 21, 3, 99)
    print(diamond(n))

elif route == "mandelbrot":
    w = safe_int(params.get("width", ""), 100, 10, 200)
    h = safe_int(params.get("height", ""), 40, 5, 80)
    print("Mandelbrot Set")
    print("=" * w)
    print("x: [-2.0, 1.0]  y: [-1.1, 1.1]  max_iter: 30")
    print("resolution: " + str(w) + "x" + str(h) + "  charset: .+*#@")
    print("")
    print(mandelbrot_render(w, h))
    print("")
    print("Compiled Python -> WASM | molt on Cloudflare Workers")

elif route == "sort":
    data = params.get("data", "42,17,93,8,55,3,71,29,64,11")
    nums = sort_data(data)
    before_parts = [str(n) for n in nums]
    before_str = ", ".join(before_parts)
    nums.sort()
    after_parts = [str(n) for n in nums]
    after_str = ", ".join(after_parts)
    print("Sort")
    print("=" * 40)
    print("")
    print("  before = [" + before_str + "]")
    print("  after  = [" + after_str + "]")
    print("  count  = " + str(len(nums)) + " elements")
    print("")
    print("Compiled Python on Cloudflare Workers")

elif route == "fizzbuzz":
    n = safe_int(parts[1] if len(parts) > 1 else "", 100, 1, 10000)
    print(fizzbuzz(n))

elif route == "pi":
    n = safe_int(parts[1] if len(parts) > 1 else "", 100000, 1, 1000000)
    total = 0.0
    for i in range(n):
        total += ((-1.0) ** i) / (2.0 * i + 1.0)
    approx = total * 4.0
    actual = 3.14159265358979323846
    error = approx - actual
    if error < 0.0:
        error = -error
    print("Pi Approximation (Leibniz Series)")
    print("=" * 40)
    print("")
    print("  terms    = " + fmt_big(n))
    print("  pi       ~= " + str(approx))
    print("  actual   = 3.14159265358979323846")
    print("  error    = " + str(error))
    print("")
    print("Compiled Python on Cloudflare Workers")

elif route == "bench":
    print("Benchmark Suite")
    print("=" * 50)
    print("")
    print("Running fib(1000) + primes(10000) + mandelbrot(80x30)")
    print("")

    # fib
    fib_result = fibonacci(1000)
    fib_digits = len(str(fib_result))
    print("  [1/3] fib(1000)         = " + str(fib_digits) + " digits   OK")

    # primes
    prime_list = primes_up_to(10000)
    prime_count = len(prime_list)
    print("  [2/3] primes(10000)     = " + str(prime_count) + " primes   OK")

    # mandelbrot
    mb = mandelbrot_render(80, 30)
    mb_chars = len(mb)
    print("  [3/3] mandelbrot(80x30) = " + str(mb_chars) + " chars    OK")

    print("")
    print("All benchmarks completed.")
    print("Total time is in the x-molt-elapsed-ms response header.")
    print("")
    print("Note: This is compiled Python, not interpreted.")
    print("Binary size: 2.9 MB gzip. Runs on Cloudflare free tier.")

else:
    print("         __  __       _ _")
    print("        |  \\/  | ___ | | |_")
    print("        | |\\/| |/ _ \\| | __|")
    print("        | |  | | (_) | | |_")
    print("        |_|  |_|\\___/|_|\\__|")
    print("")
    print("  Python compiled to WASM, running at the edge.")
    print("")
    print("=" * 52)
    print("  Not interpreted. Not transpiled. Compiled.")
    print("  Binary: 2.9 MB gzip | Platform: Cloudflare Workers")
    print("=" * 52)
    print("")
    print("  Endpoints:")
    print("")
    print("    /fib/N            Fibonacci numbers (N up to 10000)")
    print("    /primes/N         Find primes up to N (max 100000)")
    print("    /mandelbrot       ASCII Mandelbrot set")
    print("    /diamond/N        ASCII diamond pattern")
    print("    /sort?data=5,3,1  Sort a list of numbers")
    print("    /fizzbuzz/N       Classic FizzBuzz")
    print("    /pi/N             Approximate pi (Leibniz, N terms)")
    print("    /bench            Run benchmark suite")
    print("")
    print("  Example:")
    print("    curl https://molt-python-demo.pena-ch.workers.dev/fib/100")
    print("    curl https://molt-python-demo.pena-ch.workers.dev/mandelbrot")
    print("    curl https://molt-python-demo.pena-ch.workers.dev/bench")
    print("")
    print("  Source: github.com/pena-ch/molt")
