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

def mandelbrot_render(width: int = 80, height: int = 30,
                      cx: float = -0.5, cy: float = 0.0,
                      zoom: float = 1.0, max_iter: int = 60) -> str:
    chars: str = " .'`^\",:;Il!i><~+_-?][}{1)(|\\/tfjrxnuvczXYUJCLQ0OZmwqpdbkhao*#MW&8%B@$"
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
        except ValueError:
            bad.append(s)
        except TypeError:
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
    except ValueError:
        return default
    except TypeError:
        return default
    if v < lo:
        return lo
    if v > hi:
        return hi
    return v

def safe_float(s, default, lo=-1e15, hi=1e15):
    try:
        v = float(s)
    except ValueError:
        return default
    except TypeError:
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
        parts.append(s[len(s) - 3:])
        s = s[:len(s) - 3]
    parts.append(s)
    parts.reverse()
    return ",".join(parts)

def truncate_num(s, digits=60):
    if len(s) <= digits:
        return s
    return s[:digits] + "... (" + fmt_big(len(s)) + " digits)"

def _join_ints(lst):
    result = []
    i = 0
    while i < len(lst):
        result.append(str(lst[i]))
        i = i + 1
    return ", ".join(result)

# --- JSON parser/serializer ---

def _json_parse(s, i):
    while i < len(s) and s[i] in " \t\n\r":
        i = i + 1
    if i >= len(s):
        return None, i
    c = s[i]
    if c == '"':
        return _json_parse_str(s, i)
    if c == '{':
        return _json_parse_obj(s, i)
    if c == '[':
        return _json_parse_arr(s, i)
    if c == 't' and s[i:i+4] == "true":
        return True, i + 4
    if c == 'f' and s[i:i+5] == "false":
        return False, i + 5
    if c == 'n' and s[i:i+4] == "null":
        return None, i + 4
    return _json_parse_num(s, i)

def _json_parse_str(s, i):
    i = i + 1
    out = ""
    while i < len(s) and s[i] != '"':
        if s[i] == '\\':
            i = i + 1
            if s[i] == 'n':
                out = out + "\n"
            elif s[i] == 't':
                out = out + "\t"
            else:
                out = out + s[i]
        else:
            out = out + s[i]
        i = i + 1
    return out, i + 1

def _json_parse_num(s, i):
    start = i
    if i < len(s) and s[i] == '-':
        i = i + 1
    while i < len(s) and s[i].isdigit():
        i = i + 1
    is_float = False
    if i < len(s) and s[i] == '.':
        is_float = True
        i = i + 1
        while i < len(s) and s[i].isdigit():
            i = i + 1
    raw = s[start:i]
    if is_float:
        return float(raw), i
    return int(raw), i

def _json_skip_ws(s, i):
    while i < len(s) and s[i] in " \t\n\r":
        i = i + 1
    return i

def _json_parse_obj(s, i):
    i = _json_skip_ws(s, i + 1)
    obj = {}
    if i < len(s) and s[i] == '}':
        return obj, i + 1
    while i < len(s):
        i = _json_skip_ws(s, i)
        key, i = _json_parse_str(s, i)
        i = _json_skip_ws(s, i)
        i = i + 1  # skip ':'
        val, i = _json_parse(s, i)
        obj[key] = val
        i = _json_skip_ws(s, i)
        if i < len(s) and s[i] == ',':
            i = i + 1
        else:
            break
    return obj, i + 1

def _json_parse_arr(s, i):
    i = _json_skip_ws(s, i + 1)
    arr = []
    if i < len(s) and s[i] == ']':
        return arr, i + 1
    while i < len(s):
        val, i = _json_parse(s, i)
        arr.append(val)
        i = _json_skip_ws(s, i)
        if i < len(s) and s[i] == ',':
            i = i + 1
        else:
            break
    return arr, i + 1

def _json_dumps(obj, indent=0):
    sp = "  " * indent
    sp1 = "  " * (indent + 1)
    if obj is None:
        return "null"
    if obj is True:
        return "true"
    if obj is False:
        return "false"
    if type(obj) == int or type(obj) == float:
        return str(obj)
    if type(obj) == str:
        return '"' + obj.replace('\\', '\\\\').replace('"', '\\"').replace('\n', '\\n') + '"'
    if type(obj) == list:
        if len(obj) == 0:
            return "[]"
        items = []
        idx = 0
        while idx < len(obj):
            items.append(sp1 + _json_dumps(obj[idx], indent + 1))
            idx = idx + 1
        return "[\n" + ",\n".join(items) + "\n" + sp + "]"
    if type(obj) == dict:
        if len(obj) == 0:
            return "{}"
        items = []
        keys = list(obj.keys())
        idx = 0
        while idx < len(keys):
            k = keys[idx]
            items.append(sp1 + '"' + str(k) + '": ' + _json_dumps(obj[k], indent + 1))
            idx = idx + 1
        return "{\n" + ",\n".join(items) + "\n" + sp + "}"
    return str(obj)

# --- Template engine ---

def _tmpl_render(template, ctx):
    out = ""
    i = 0
    while i < len(template):
        if template[i:i+2] == "{{":
            end = template.find("}}", i + 2)
            varname = template[i+2:end].strip()
            val = ctx.get(varname, "")
            out = out + str(val)
            i = end + 2
        elif template[i:i+2] == "{%":
            end_tag = template.find("%}", i + 2)
            directive = template[i+2:end_tag].strip()
            # parse "for VAR in LIST"
            words = directive.split()
            if words[0] == "for":
                varname = words[1]
                listname = words[3]
                body_start = end_tag + 2
                endfor = template.find("{% endfor %}", body_start)
                body = template[body_start:endfor]
                items = ctx.get(listname, [])
                idx = 0
                while idx < len(items):
                    child = {}
                    keys = list(ctx.keys())
                    ki = 0
                    while ki < len(keys):
                        child[keys[ki]] = ctx[keys[ki]]
                        ki = ki + 1
                    child[varname] = items[idx]
                    out = out + _tmpl_render(body, child)
                    idx = idx + 1
                i = endfor + len("{% endfor %}")
            else:
                out = out + template[i]
                i = i + 1
        else:
            out = out + template[i]
            i = i + 1
    return out

# --- CSV parser + stats ---

def _csv_parse(text):
    rows = []
    lines = text.strip().split("\n")
    idx = 0
    while idx < len(lines):
        cells = lines[idx].split(",")
        clean = []
        ci = 0
        while ci < len(cells):
            clean.append(cells[ci].strip())
            ci = ci + 1
        rows.append(clean)
        idx = idx + 1
    return rows

def _stats(nums):
    n = len(nums)
    if n == 0:
        return {}
    s = 0.0
    lo = nums[0]
    hi = nums[0]
    i = 0
    while i < n:
        s = s + nums[i]
        if nums[i] < lo:
            lo = nums[i]
        if nums[i] > hi:
            hi = nums[i]
        i = i + 1
    mean = s / n
    sorted_n = list(nums)
    sorted_n.sort()
    if n % 2 == 1:
        median = sorted_n[n // 2]
    else:
        median = (sorted_n[n // 2 - 1] + sorted_n[n // 2]) / 2.0
    var_sum = 0.0
    i = 0
    while i < n:
        d = nums[i] - mean
        var_sum = var_sum + d * d
        i = i + 1
    stddev = (var_sum / n) ** 0.5
    return {"min": lo, "max": hi, "mean": mean, "median": median, "stddev": stddev, "n": n}

# --- Pure-Python NFA regex ---

def _re_match(pattern, text):
    tokens = _re_tokenize(pattern)
    anchored_start = False
    anchored_end = False
    if len(tokens) > 0 and tokens[0] == ('^',):
        anchored_start = True
        tokens = tokens[1:]
    if len(tokens) > 0 and tokens[-1] == ('$',):
        anchored_end = True
        tokens = tokens[:-1]
    if anchored_start:
        m = _re_try(tokens, text, 0, anchored_end)
        return m is not None
    pos = 0
    while pos <= len(text):
        m = _re_try(tokens, text, pos, anchored_end)
        if m is not None:
            return True
        pos = pos + 1
    return False

def _re_tokenize(pattern):
    tokens = []
    i = 0
    while i < len(pattern):
        c = pattern[i]
        if c == '^':
            tokens.append(('^',))
            i = i + 1
        elif c == '$':
            tokens.append(('$',))
            i = i + 1
        elif c == '.':
            tok = ('dot',)
            i = i + 1
            if i < len(pattern) and pattern[i] in '*+?':
                tok = ('dot', pattern[i])
                i = i + 1
            tokens.append(tok)
        elif c == '[':
            end = pattern.find(']', i)
            inner = pattern[i+1:end]
            negate = False
            if inner and inner[0] == '^':
                negate = True
                inner = inner[1:]
            chars = _re_parse_class(inner)
            tok = ('class', chars, negate)
            i = end + 1
            if i < len(pattern) and pattern[i] in '*+?':
                tok = ('class', chars, negate, pattern[i])
                i = i + 1
            tokens.append(tok)
        else:
            tok = ('lit', c)
            i = i + 1
            if i < len(pattern) and pattern[i] in '*+?':
                tok = ('lit', c, pattern[i])
                i = i + 1
            tokens.append(tok)
    return tokens

def _re_parse_class(inner):
    chars = set()
    i = 0
    while i < len(inner):
        if i + 2 < len(inner) and inner[i+1] == '-':
            lo = ord(inner[i])
            hi = ord(inner[i+2])
            c = lo
            while c <= hi:
                chars.add(chr(c))
                c = c + 1
            i = i + 3
        else:
            chars.add(inner[i])
            i = i + 1
    return chars

def _re_char_match(tok, ch):
    if tok[0] == 'dot':
        return True
    if tok[0] == 'lit':
        return ch == tok[1]
    if tok[0] == 'class':
        in_set = ch in tok[1]
        return (not in_set) if tok[2] else in_set
    return False

def _re_quantifier(tok):
    if tok[0] == 'dot':
        return tok[1] if len(tok) > 1 else None
    if tok[0] == 'lit':
        return tok[2] if len(tok) > 2 else None
    if tok[0] == 'class':
        return tok[3] if len(tok) > 3 else None
    return None

def _re_try(tokens, text, start, anchored_end):
    return _re_bt(tokens, 0, text, start, anchored_end)

def _re_bt(tokens, ti, text, si, anchored_end):
    if ti == len(tokens):
        if anchored_end:
            return si if si == len(text) else None
        return si
    tok = tokens[ti]
    q = _re_quantifier(tok)
    if q is None:
        if si >= len(text):
            return None
        if _re_char_match(tok, text[si]):
            return _re_bt(tokens, ti + 1, text, si + 1, anchored_end)
        return None
    if q == '?':
        r = _re_bt(tokens, ti + 1, text, si, anchored_end)
        if r is not None:
            return r
        if si < len(text) and _re_char_match(tok, text[si]):
            return _re_bt(tokens, ti + 1, text, si + 1, anchored_end)
        return None
    if q == '*' or q == '+':
        # greedy: consume as many as possible, then backtrack
        count = 0
        pos = si
        while pos < len(text) and _re_char_match(tok, text[pos]):
            count = count + 1
            pos = pos + 1
        min_c = 1 if q == '+' else 0
        while count >= min_c:
            r = _re_bt(tokens, ti + 1, text, si + count, anchored_end)
            if r is not None:
                return r
            count = count - 1
        return None
    return None

# --- Matrix operations ---

def _matmul(A, B):
    n = len(A)
    m = len(B[0])
    p = len(B)
    C = []
    i = 0
    while i < n:
        row = []
        j = 0
        while j < m:
            s = 0.0
            k = 0
            while k < p:
                s = s + A[i][k] * B[k][j]
                k = k + 1
            row.append(s)
            j = j + 1
        C.append(row)
        i = i + 1
    return C

def _gauss_solve(A, b):
    n = len(A)
    # build augmented matrix
    M = []
    i = 0
    while i < n:
        row = []
        j = 0
        while j < n:
            row.append(float(A[i][j]))
            j = j + 1
        row.append(float(b[i]))
        M.append(row)
        i = i + 1
    # forward elimination with partial pivoting
    col = 0
    while col < n:
        # find pivot
        max_val = -1.0
        max_row = col
        r = col
        while r < n:
            v = M[r][col]
            if v < 0:
                v = -v
            if v > max_val:
                max_val = v
                max_row = r
            r = r + 1
        if max_row != col:
            M[col], M[max_row] = M[max_row], M[col]
        pivot = M[col][col]
        r = col + 1
        while r < n:
            factor = M[r][col] / pivot
            c = col
            while c <= n:
                M[r][c] = M[r][c] - factor * M[col][c]
                c = c + 1
            r = r + 1
        col = col + 1
    # back substitution
    x = []
    i = 0
    while i < n:
        x.append(0.0)
        i = i + 1
    i = n - 1
    while i >= 0:
        s = M[i][n]
        j = i + 1
        while j < n:
            s = s - M[i][j] * x[j]
            j = j + 1
        x[i] = s / M[i][i]
        i = i - 1
    return x

def _matpow(M, p):
    n = len(M)
    # identity
    result = []
    i = 0
    while i < n:
        row = []
        j = 0
        while j < n:
            if i == j:
                row.append(1.0)
            else:
                row.append(0.0)
            j = j + 1
        result.append(row)
        i = i + 1
    base = M
    while p > 0:
        if p % 2 == 1:
            result = _matmul(result, base)
        base = _matmul(base, base)
        p = p // 2
    return result

def _fmt_matrix(M, label, width=8):
    lines = []
    lines.append("  " + label + ":")
    i = 0
    while i < len(M):
        cells = []
        j = 0
        while j < len(M[i]):
            v = M[i][j]
            if type(v) == float:
                s = str(round(v * 1000) / 1000.0)
            else:
                s = str(v)
            while len(s) < width:
                s = " " + s
            cells.append(s)
            j = j + 1
        lines.append("    [" + "".join(cells) + " ]")
        i = i + 1
    return "\n".join(lines)

def _fmt_vec(v, label):
    parts = []
    i = 0
    while i < len(v):
        parts.append(str(round(v[i] * 10000) / 10000.0))
        i = i + 1
    return "  " + label + ": [" + ", ".join(parts) + "]"

# --- microGPT ---
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
    n = safe_int(parts[1] if len(parts) > 1 else "", 50, 0, 10000)
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
    print("github.com/adpena/molt")

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
        print("  primes = " + _join_ints(found))
    else:
        first5 = []
        i = 0
        while i < 5 and i < count:
            first5.append(str(found[i]))
            i = i + 1
        last5 = []
        start = count - 5
        if start < 0:
            start = 0
        i = start
        while i < count:
            last5.append(str(found[i]))
            i = i + 1
        print("  first  = " + ", ".join(first5))
        print("  last   = " + ", ".join(last5))
    print("")
    print("github.com/adpena/molt")

elif route == "diamond":
    n = safe_int(parts[1] if len(parts) > 1 else "", 21, 3, 99)
    print(diamond(n))

elif route == "mandelbrot":
    w = safe_int(params.get("width", ""), 80, 20, 160)
    h = safe_int(params.get("height", ""), 30, 10, 50)
    mi = safe_int(params.get("iter", ""), 60, 10, 300)
    cx = safe_float(params.get("cx", ""), -0.5)
    cy = safe_float(params.get("cy", ""), 0.0)
    zm = safe_float(params.get("zoom", ""), 1.0, 0.1, 1e12)
    # Preset views via /mandelbrot/N
    preset = safe_int(parts[1] if len(parts) > 1 else "", 0, 0, 9)
    if preset == 1:
        cx = -0.7435
        cy = 0.1314
        zm = 100.0
        mi = 120
    elif preset == 2:
        cx = 0.360284
        cy = -0.641216
        zm = 150.0
        mi = 120
    elif preset == 3:
        cx = -0.16
        cy = 1.0405
        zm = 80.0
        mi = 120
    elif preset == 4:
        cx = -1.25066
        cy = 0.02012
        zm = 200.0
        mi = 150
    elif preset == 5:
        cx = -0.745428
        cy = 0.113009
        zm = 300.0
        mi = 180

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
    print("github.com/adpena/molt")

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
    before = []
    i = 0
    while i < len(nums):
        before.append(str(nums[i]))
        i = i + 1
    nums.sort()
    after = []
    i = 0
    while i < len(nums):
        after.append(str(nums[i]))
        i = i + 1
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
    print("github.com/adpena/molt")

elif route == "fizzbuzz":
    n = safe_int(parts[1] if len(parts) > 1 else "", 100, 1, 10000)
    print("FizzBuzz (1 to " + str(n) + ")")
    print("=" * 40)
    print("")
    print(fizzbuzz(n))

elif route == "pi":
    n = safe_int(parts[1] if len(parts) > 1 else "", 10000, 1, 500000)
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
    print("github.com/adpena/molt")

elif route == "bench":
    print("Benchmark Suite")
    print("=" * 50)
    print("")
    print("Running fib(500) + primes(1000) + mandelbrot(40x15)")
    print("")

    fib_result = fibonacci(500)
    fib_digits = len(str(fib_result))
    print("  [1/3] fib(500)          = " + str(fib_digits) + " digits   OK")

    prime_list = primes_up_to(1000)
    prime_count = len(prime_list)
    print("  [2/3] primes(1000)      = " + str(prime_count) + " primes   OK")

    mb = mandelbrot_render(40, 15, -0.5, 0.0, 1.0, 40)
    mb_chars = len(mb)
    print("  [3/3] mandelbrot(40x15) = " + str(mb_chars) + " chars    OK")

    print("")
    print("All benchmarks completed in x-molt-elapsed-ms (see header).")
    print("")
    print("This is compiled Python, not interpreted.")
    print("Binary: 2.8 MB gzip | Cloudflare Workers free tier.")

elif route == "json":
    input_json = '{"name": "alice", "age": 30, "scores": [95, 87, 92], "active": true}'
    parsed, _ = _json_parse(input_json, 0)
    # transform: uppercase name, add computed field
    parsed["name"] = parsed["name"].upper()
    scores = parsed["scores"]
    total = 0.0
    si = 0
    while si < len(scores):
        total = total + scores[si]
        si = si + 1
    parsed["average"] = round(total / len(scores) * 100) / 100.0
    output_json = _json_dumps(parsed)
    print("JSON Parse + Transform + Serialize")
    print("=" * 50)
    print("")
    print("  Input:")
    print("    " + input_json)
    print("")
    print("  Transforms: uppercase name, compute average score")
    print("")
    print("  Output:")
    lines = output_json.split("\n")
    li = 0
    while li < len(lines):
        print("    " + lines[li])
        li = li + 1
    print("")
    print("  Pure-Python recursive descent parser (no json module)")
    print("")
    print("github.com/adpena/molt")

elif route == "template":
    tmpl = "<h1>{{ title }}</h1>\n<ul>\n{% for item in items %}  <li>{{ item }}</li>\n{% endfor %}</ul>\n<p>Count: {{ count }}</p>"
    ctx = {
        "title": "Molt Demo",
        "items": ["Fibonacci", "Mandelbrot", "Primes", "FizzBuzz"],
        "count": "4"
    }
    rendered = _tmpl_render(tmpl, ctx)
    print("Template Engine")
    print("=" * 50)
    print("")
    print("  Template:")
    tlines = tmpl.split("\n")
    ti = 0
    while ti < len(tlines):
        print("    " + tlines[ti])
        ti = ti + 1
    print("")
    print("  Context:")
    print('    title = "Molt Demo"')
    print('    items = ["Fibonacci", "Mandelbrot", "Primes", "FizzBuzz"]')
    print('    count = "4"')
    print("")
    print("  Rendered:")
    rlines = rendered.split("\n")
    ri = 0
    while ri < len(rlines):
        print("    " + rlines[ri])
        ri = ri + 1
    print("")
    print("  Supports {{ var }} and {% for x in list %}...{% endfor %}")
    print("")
    print("github.com/adpena/molt")

elif route == "csv":
    csv_data = """city,country,population_m,gdp_b
Tokyo,Japan,13.96,1920
Delhi,India,11.03,294
Shanghai,China,24.28,690
Sao Paulo,Brazil,12.33,430
Mexico City,Mexico,9.21,411
Cairo,Egypt,9.54,135
Mumbai,India,12.48,368
Beijing,China,21.54,640
Dhaka,Bangladesh,8.91,110
Osaka,Japan,2.75,681"""
    rows = _csv_parse(csv_data)
    header = rows[0]
    data_rows = rows[1:]
    pop_vals = []
    gdp_vals = []
    ri = 0
    while ri < len(data_rows):
        pop_vals.append(float(data_rows[ri][2]))
        gdp_vals.append(float(data_rows[ri][3]))
        ri = ri + 1
    pop_st = _stats(pop_vals)
    gdp_st = _stats(gdp_vals)
    print("CSV Parsing + Descriptive Statistics")
    print("=" * 50)
    print("")
    print("  Dataset: 10 World Cities")
    print("  " + "-" * 46)
    ri = 0
    while ri < len(data_rows):
        r = data_rows[ri]
        name = r[0]
        while len(name) < 14:
            name = name + " "
        print("    " + name + r[1] + " | pop " + r[2] + "M | GDP $" + r[3] + "B")
        ri = ri + 1
    print("")
    print("  Population (millions):")
    print("    min    = " + str(pop_st["min"]))
    print("    max    = " + str(pop_st["max"]))
    print("    mean   = " + str(round(pop_st["mean"] * 100) / 100.0))
    print("    median = " + str(round(pop_st["median"] * 100) / 100.0))
    print("    stddev = " + str(round(pop_st["stddev"] * 100) / 100.0))
    print("")
    print("  GDP ($ billions):")
    print("    min    = " + str(gdp_st["min"]))
    print("    max    = " + str(gdp_st["max"]))
    print("    mean   = " + str(round(gdp_st["mean"] * 100) / 100.0))
    print("    median = " + str(round(gdp_st["median"] * 100) / 100.0))
    print("    stddev = " + str(round(gdp_st["stddev"] * 100) / 100.0))
    print("")
    print("github.com/adpena/molt")

elif route == "hash":
    import hashlib
    msg = params.get("msg", "Hello from Molt!")
    msg_bytes = msg.encode("utf-8")
    md5 = hashlib.md5(msg_bytes).hexdigest()
    sha1 = hashlib.sha1(msg_bytes).hexdigest()
    sha256 = hashlib.sha256(msg_bytes).hexdigest()
    sha512 = hashlib.sha512(msg_bytes).hexdigest()
    print("Cryptographic Hashing")
    print("=" * 50)
    print("")
    print("  Input:  " + '"' + msg + '"')
    print("")
    print("  MD5:    " + md5)
    print("  SHA1:   " + sha1)
    print("  SHA256: " + sha256)
    print("  SHA512: " + sha512)
    print("")
    print("  hashlib backed by Rust intrinsics in Molt")
    print("")
    print("github.com/adpena/molt")

elif route == "regex":
    pattern = params.get("pattern", "^[a-z]+e$")
    corpus = [
        "the", "be", "to", "of", "and", "a", "in", "that", "have", "I",
        "it", "for", "not", "on", "with", "he", "as", "you", "do", "at",
        "this", "but", "his", "by", "from", "they", "we", "say", "her",
        "she", "or", "an", "will", "my", "one", "all", "would", "there",
        "their", "what", "so", "up", "out", "if", "about", "who", "get",
        "which", "go", "me", "when", "make", "can", "like", "time", "no",
        "just", "him", "know", "take", "people", "into", "year", "your",
        "good", "some", "could", "them", "see", "other", "than", "then",
        "now", "look", "only", "come", "its", "over", "think", "also",
        "back", "after", "use", "two", "how", "our", "work", "first",
        "well", "way", "even", "new", "want", "because", "any", "these",
        "give", "day", "most"
    ]
    matches = []
    wi = 0
    while wi < len(corpus):
        if _re_match(pattern, corpus[wi]):
            matches.append(corpus[wi])
        wi = wi + 1
    print("Regex Pattern Matching (Pure-Python NFA)")
    print("=" * 50)
    print("")
    print("  Pattern: " + pattern)
    print("  Corpus:  " + str(len(corpus)) + " common English words")
    print("")
    print("  Matches (" + str(len(matches)) + "):")
    mi = 0
    while mi < len(matches):
        print("    - " + matches[mi])
        mi = mi + 1
    print("")
    print("  Supports: . * + ? ^ $ [abc] [a-z] [^abc]")
    print("  Try: /regex?pattern=^th")
    print("")
    print("github.com/adpena/molt")

elif route == "matrix":
    n = safe_int(parts[1] if len(parts) > 1 else "", 4, 2, 8)
    # build well-conditioned NxN matrix
    A = []
    i = 0
    while i < n:
        row = []
        j = 0
        while j < n:
            if i == j:
                row.append(float(n + i + 1))
            else:
                row.append(float((i + 1) * (j + 1) % (n + 1)))
            j = j + 1
        A.append(row)
        i = i + 1
    # b vector
    b = []
    i = 0
    while i < n:
        s = 0.0
        j = 0
        while j < n:
            s = s + A[i][j]
            j = j + 1
        b.append(s)
        i = i + 1
    # solve Ax = b (should give x = [1,1,...,1])
    x = _gauss_solve(A, b)
    # matrix power
    C = _matpow(A, 2)
    # rotation demo (2x2 embedded)
    import math
    angle = 3.14159265 / 4.0  # 45 degrees
    cos_a = math.cos(angle)
    sin_a = math.sin(angle)
    R = [[cos_a, -sin_a], [sin_a, cos_a]]
    R8 = _matpow(R, 8)
    print("Matrix Operations (" + str(n) + "x" + str(n) + ")")
    print("=" * 50)
    print("")
    print(_fmt_matrix(A, "A (" + str(n) + "x" + str(n) + ")"))
    print("")
    print("  b = row sums of A")
    print(_fmt_vec(b, "b"))
    print("")
    print("  Gaussian elimination with partial pivoting:")
    print(_fmt_vec(x, "x (should be all 1.0)"))
    print("")
    print(_fmt_matrix(C, "A^2"))
    print("")
    print("  Rotation demo: R(45 deg) raised to 8th power = R(360 deg) = I")
    print(_fmt_matrix(R8, "R(45)^8"))
    print("")
    print("  /matrix/N where N = 2..8")
    print("")
    print("github.com/adpena/molt")

else:
    if route:
        print("404 Not Found: /" + route)
        print("")
    print("   __  __       _ _   _")
    print("  |  \\/  | ___ | | |_| | __ _ _ __   __ _")
    print("  | |\\/| |/ _ \\| | __| |/ _` | '_ \\ / _` |")
    print("  | |  | | (_) | | |_| | (_| | | | | (_| |")
    print("  |_|  |_|\\___/|_|\\__|_|\\__,_|_| |_|\\__, |")
    print("                                     |___/")
    print("")
    print("  Python compiled to WebAssembly.")
    print("  2.8 MB gzip. Cloudflare Workers, Free Tier.")
    print("")
    print("  Endpoints:")
    print("")
    print("    /fib/N              Fibonacci (N up to 10,000)")
    print("    /primes/N           Primes up to N (max 50,000)")
    print("    /mandelbrot         ASCII Mandelbrot set")
    print("    /mandelbrot/1-5     Zoom presets")
    print("    /diamond/N          ASCII diamond pattern")
    print("    /sort?data=5,3,1    Sort numbers")
    print("    /fizzbuzz/N         FizzBuzz")
    print("    /pi/N               Approximate pi (Leibniz series)")
    print("    /bench              Run benchmark suite")
    print("    /json               JSON parse + transform + serialize")
    print("    /template           Minimal template engine demo")
    print("    /csv                CSV parsing + descriptive statistics")
    print("    /hash?msg=text      Cryptographic hashing (md5/sha)")
    print("    /regex?pattern=...  Pattern matching (pure-Python NFA)")
    print("    /matrix/N           Matrix multiply + Gaussian elimination")
    print("")
    print("  Try:")
    print("    curl https://molt-python-demo.adpena.workers.dev/mandelbrot")
    print("    curl https://molt-python-demo.adpena.workers.dev/fib/500")
    print("    curl https://molt-python-demo.adpena.workers.dev/bench")
    print("    curl https://molt-python-demo.adpena.workers.dev/json")
    print("    curl https://molt-python-demo.adpena.workers.dev/regex?pattern=^th")
    print("")
    print("  github.com/adpena  |  adpena@gmail.com")
    if route:
        sys.exit(1)
