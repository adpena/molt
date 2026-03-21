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
    result = []
    n = 2
    while n <= limit:
        if is_prime(n):
            result.append(n)
        n = n + 1
    return result

def diamond(size=9):
    lines = []
    for i in range(size):
        d = i if i < size // 2 + 1 else size - 1 - i
        lines.append(" " * (size // 2 - d) + "*" * (2 * d + 1))
    return "\n".join(lines)

def mandelbrot_render(width: int = 120, height: int = 50,
                      cx: float = -0.5, cy: float = 0.0,
                      zoom: float = 1.0, max_iter: int = 80) -> str:
    chars: str = " .`'\"~:;!>+r}xz&8#@"
    scale: float = 3.0 / (zoom * width)
    x_off: float = cx - scale * width / 2.0
    y_off: float = cy - scale * height / 2.0
    out: list = []
    row: int = 0
    while row < height:
        y0: float = y_off + scale * row
        col: int = 0
        line: str = ""
        while col < width:
            x0: float = x_off + scale * col
            x: float = 0.0
            y: float = 0.0
            it: int = 0
            while it < max_iter:
                xx: float = x * x
                yy: float = y * y
                if xx + yy > 4.0:
                    break
                y = 2.0 * x * y + y0
                x = xx - yy + x0
                it = it + 1
            ci: int = it * (len(chars) - 1) // max_iter
            if ci >= len(chars):
                ci = len(chars) - 1
            line = line + chars[ci]
            col = col + 1
        out.append(line)
        row = row + 1
    return "\n".join(out)

def sort_data(data_str):
    nums = []
    bad = []
    for p in data_str.split(","):
        s = p.strip()
        if not s:
            continue
        try:
            nums.append(int(s))
        except (ValueError, TypeError):
            bad.append(s)
    return nums, bad

def fizzbuzz(n):
    lines = []
    for i in range(1, n + 1):
        if i % 15 == 0: lines.append("FizzBuzz")
        elif i % 3 == 0: lines.append("Fizz")
        elif i % 5 == 0: lines.append("Buzz")
        else: lines.append(str(i))
    return "\n".join(lines)

def safe_int(s, default, lo=0, hi=1000000):
    try:
        v = int(s)
    except (ValueError, TypeError):
        return default
    if v < lo:
        return lo
    if v > hi:
        return hi
    return v

def safe_float(s, default, lo=-1e15, hi=1e15):
    try:
        v = float(s)
    except (ValueError, TypeError):
        return default
    if v < lo:
        return lo
    if v > hi:
        return hi
    return v

def fmt_big(n):
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

def truncate_num(s, digits=60):
    if len(s) <= digits:
        return s
    return s[:digits] + "... (" + fmt_big(len(s)) + " digits)"

# --- Request parsing ---
import sys
path = sys.argv[1] if len(sys.argv) > 1 else "/"
query = sys.argv[2] if len(sys.argv) > 2 else ""

params = {}
if query:
    for part in query.split("&"):
        if "=" in part:
            k, v = part.split("=", 1)
            params[k] = v

parts = path.strip("/").split("/")
route = parts[0] if parts else ""

# --- Routes ---

if route == "fib":
    n = safe_int(parts[1] if len(parts) > 1 else "", 100, 0, 10000)
    result = fibonacci(n)
    result_str = str(result)
    print("Fibonacci")
    print("=" * 40)
    print("")
    print("  n      = " + fmt_big(n))
    print("  fib(n) = " + truncate_num(result_str))
    if len(result_str) > 3:
        print("  digits = " + fmt_big(len(result_str)))
    print("")
    print("Compiled Python -> WASM | molt")

elif route == "primes":
    limit = safe_int(parts[1] if len(parts) > 1 else "", 10000, 2, 50000)
    found = primes_up_to(limit)
    count = len(found)
    print("Prime Numbers")
    print("=" * 40)
    print("")
    print("  range  = 2 to " + fmt_big(limit))
    print("  count  = " + fmt_big(count))
    print("")
    if count <= 20:
        print("  primes = " + ", ".join(str(p) for p in found))
    else:
        first5 = ", ".join(str(p) for p in found[:5])
        last5 = ", ".join(str(p) for p in found[-5:])
        print("  first  = " + first5)
        print("  last   = " + last5)
    print("")
    print("Compiled Python -> WASM | molt")

elif route == "diamond":
    n = safe_int(parts[1] if len(parts) > 1 else "", 21, 3, 99)
    print(diamond(n))

elif route == "mandelbrot":
    w = safe_int(params.get("width", ""), 120, 20, 200)
    h = safe_int(params.get("height", ""), 50, 10, 80)
    mi = safe_int(params.get("iter", ""), 80, 10, 200)
    cx = safe_float(params.get("cx", ""), -0.5)
    cy = safe_float(params.get("cy", ""), 0.0)
    zm = safe_float(params.get("zoom", ""), 1.0, 0.1, 1e12)
    # Preset views via /mandelbrot/N
    preset = safe_int(parts[1] if len(parts) > 1 else "", 0, 0, 9)
    if preset == 1:
        cx = -0.7435
        cy = 0.1314
        zm = 200.0
        mi = 150
    elif preset == 2:
        cx = 0.360284
        cy = -0.641216
        zm = 500.0
        mi = 150
    elif preset == 3:
        cx = -0.16
        cy = 1.0405
        zm = 100.0
        mi = 120
    elif preset == 4:
        cx = -1.25066
        cy = 0.02012
        zm = 1000.0
        mi = 180
    elif preset == 5:
        cx = -0.745428
        cy = 0.113009
        zm = 5000.0
        mi = 200

    print("Mandelbrot Set")
    print("=" * w)
    if preset > 0:
        print("preset: " + str(preset) + "  center: (" + str(cx) + ", " + str(cy) + ")  zoom: " + str(zm) + "x")
    else:
        print("center: (-0.5, 0.0)  zoom: 1x  max_iter: " + str(mi))
    print("resolution: " + str(w) + "x" + str(h))
    print("")
    print(mandelbrot_render(w, h, cx, cy, zm, mi))
    print("")
    print("Try: /mandelbrot/1 through /mandelbrot/5 for deep zooms")
    print("Or:  /mandelbrot?cx=-0.74&cy=0.13&zoom=200&iter=150")
    print("")
    print("Compiled Python -> WASM | molt on Cloudflare Workers")

elif route == "sort":
    data = params.get("data", "")
    if not data and len(parts) > 1:
        data = parts[1]
    if not data:
        data = "42,17,93,8,55,3,71,29,64,11"
    nums, bad = sort_data(data)
    if len(nums) > 1000:
        print("Error: too many elements (max 1000)")
        sys.exit(1)
    before = [str(n) for n in nums]
    nums.sort()
    after = [str(n) for n in nums]
    print("Sort")
    print("=" * 40)
    print("")
    print("  input  = " + data)
    print("  before = [" + ", ".join(before) + "]")
    print("  after  = [" + ", ".join(after) + "]")
    print("  count  = " + str(len(nums)) + " elements")
    if bad:
        print("  skipped = " + ", ".join(bad) + " (non-numeric)")
    print("")
    print("Compiled Python -> WASM | molt")

elif route == "fizzbuzz":
    n = safe_int(parts[1] if len(parts) > 1 else "", 100, 1, 10000)
    print("FizzBuzz (1 to " + str(n) + ")")
    print("=" * 40)
    print("")
    print(fizzbuzz(n))

elif route == "pi":
    n = safe_int(parts[1] if len(parts) > 1 else "", 100000, 1, 500000)
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
    print("  pi       = " + str(approx))
    print("  actual   = 3.14159265358979323846")
    print("  error    = " + str(error))
    print("")
    print("Compiled Python -> WASM | molt")

elif route == "bench":
    print("Benchmark Suite")
    print("=" * 50)
    print("")
    print("Running fib(1000) + primes(10000) + mandelbrot(100x40)")
    print("")

    fib_result = fibonacci(1000)
    fib_digits = len(str(fib_result))
    print("  [1/3] fib(1000)          = " + str(fib_digits) + " digits   OK")

    prime_list = primes_up_to(10000)
    prime_count = len(prime_list)
    print("  [2/3] primes(10000)      = " + str(prime_count) + " primes   OK")

    mb = mandelbrot_render(100, 40, -0.5, 0.0, 1.0, 80)
    mb_chars = len(mb)
    print("  [3/3] mandelbrot(100x40) = " + str(mb_chars) + " chars    OK")

    print("")
    print("All benchmarks completed in x-molt-elapsed-ms (see header).")
    print("")
    print("This is compiled Python, not interpreted.")
    print("Binary: 2.9 MB gzip | Cloudflare Workers free tier.")

else:
    if route:
        print("404 Not Found: /" + route)
        print("")
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
    print("    /fib/N              Fibonacci (N up to 10,000)")
    print("    /primes/N           Primes up to N (max 50,000)")
    print("    /mandelbrot         ASCII Mandelbrot set (120x50)")
    print("    /mandelbrot/1       Deep zoom preset 1")
    print("    /mandelbrot/2       Deep zoom preset 2")
    print("    /mandelbrot/3       Deep zoom preset 3")
    print("    /diamond/N          ASCII diamond pattern")
    print("    /sort?data=5,3,1    Sort numbers")
    print("    /fizzbuzz/N         Classic FizzBuzz")
    print("    /pi/N               Approximate pi (N terms)")
    print("    /bench              Run benchmark suite")
    print("")
    print("  Examples:")
    print("    curl .../fib/500")
    print("    curl .../mandelbrot")
    print("    curl .../mandelbrot/1")
    print("    curl .../bench")
    if route:
        sys.exit(1)
