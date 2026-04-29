"""
Real-world benchmark suite for molt vs CPython.

Tests patterns that appear in actual Python applications, not microbenchmarks.
Each benchmark targets ~100ms on CPython for accurate measurement.

Uses while-loop iteration in function bodies (molt's for-range in function
scope hits a known Cranelift codegen trap; tracked separately).

Usage:
    python3 bench/real_world_bench.py          # CPython baseline
    molt build bench/real_world_bench.py ...   # molt

Compilation notes (molt native backend):
    - json/re module imports compile but depend on stdlib batch compilation
    - for-range inside function bodies crashes at runtime (SIGILL) -- using while loops
    - augmented assign (+=) with float inside for-loops causes compile panic -- using explicit add
"""

import time


def bench(name, func, *args):
    """Run a benchmark, report timing, return result for correctness check."""
    # warmup
    func(*args)
    # measure
    start = time.time()
    result = func(*args)
    elapsed = (time.time() - start) * 1000
    print(f"  {name:45s} {elapsed:8.2f}ms  checksum={result}")
    return result


# ---------------------------------------------------------------------------
# 1. JSON roundtrip
#    Parse a ~10KB JSON string, modify values, serialize back.
#    Tests: dict access, string ops, nested structures, type coercion.
# ---------------------------------------------------------------------------


def _build_json_string():
    """Build a ~10KB JSON string by hand (no json module needed for construction)."""
    entries = []
    i = 0
    while i < 200:
        entries.append(
            '"item_'
            + str(i)
            + '": {"id": '
            + str(i)
            + ', "name": "widget_'
            + str(i)
            + '", "price": '
            + str(i * 1.5)
            + ', "tags": ["tag_a", "tag_b", "tag_'
            + str(i % 10)
            + '"], "nested": {"x": '
            + str(i)
            + ', "y": '
            + str(i * 2)
            + "}}"
        )
        i = i + 1
    return "{" + ", ".join(entries) + "}"


def json_roundtrip():
    """Parse JSON, modify values, serialize back."""
    import json

    raw = _build_json_string()
    total_price = 0.0
    tag_counts = {}
    pass_num = 0
    while pass_num < 250:
        data = json.loads(raw)
        # modify: bump every price by 10%, collect tags
        keys = list(data.keys())
        ki = 0
        while ki < len(keys):
            key = keys[ki]
            item = data[key]
            item["price"] = item["price"] * 1.1
            total_price = total_price + item["price"]
            tags = item["tags"]
            ti = 0
            while ti < len(tags):
                tag = tags[ti]
                if tag in tag_counts:
                    tag_counts[tag] = tag_counts[tag] + 1
                else:
                    tag_counts[tag] = 1
                ti = ti + 1
            item["nested"]["x"] = item["nested"]["x"] + 1
            ki = ki + 1
        # serialize back
        _ = json.dumps(data)
        pass_num = pass_num + 1
    return int(total_price * 100) ^ len(tag_counts)


# ---------------------------------------------------------------------------
# 2. Regex matching
#    Compile 5 patterns, match against 10K strings.
#    Tests: re module, string iteration, list building.
# ---------------------------------------------------------------------------


def regex_matching():
    """Compile patterns, match against many strings."""
    import re

    patterns = [
        re.compile(r"\b[A-Z][a-z]+\b"),
        re.compile(r"\d{3}-\d{4}"),
        re.compile(r"[a-z]+@[a-z]+\.[a-z]+"),
        re.compile(r"^\d+\.\d+$"),
        re.compile(r"(foo|bar|baz)\w*"),
    ]
    # generate test strings
    strings = []
    i = 0
    while i < 10000:
        mod = i % 5
        if mod == 0:
            strings.append("Hello world item " + str(i))
        elif mod == 1:
            strings.append("555-" + str(i).zfill(4))
        elif mod == 2:
            strings.append("user" + str(i) + "@example.com")
        elif mod == 3:
            strings.append(str(i) + "." + str(i * 7 % 100))
        else:
            strings.append("foobar_" + str(i) + "_bazqux")
        i = i + 1
    match_count = 0
    pass_num = 0
    while pass_num < 15:
        si = 0
        while si < len(strings):
            s = strings[si]
            pi = 0
            while pi < len(patterns):
                if patterns[pi].search(s):
                    match_count = match_count + 1
                pi = pi + 1
            si = si + 1
        pass_num = pass_num + 1
    return match_count


# ---------------------------------------------------------------------------
# 3. CSV processing
#    Parse 1000 rows, compute column statistics (sum, mean, max).
#    Tests: string split, float conversion, accumulation.
# ---------------------------------------------------------------------------


def _build_csv_data():
    """Build CSV data as a single string."""
    lines = ["name,value_a,value_b,value_c"]
    i = 0
    while i < 1000:
        a = (i * 7 + 13) % 997
        b = (i * 11 + 23) % 499
        c = (i * 3 + 7) % 251
        b_frac = str(b % 100)
        if len(b_frac) == 1:
            b_frac = "0" + b_frac
        c_frac = str(c % 100)
        if len(c_frac) == 1:
            c_frac = "0" + c_frac
        a_frac = str(a % 100)
        if len(a_frac) == 1:
            a_frac = "0" + a_frac
        lines.append(
            "row_"
            + str(i)
            + ","
            + str(a)
            + "."
            + b_frac
            + ","
            + str(b)
            + "."
            + c_frac
            + ","
            + str(c)
            + "."
            + a_frac
        )
        i = i + 1
    return "\n".join(lines)


def csv_processing():
    """Parse CSV rows, compute column statistics."""
    csv_text = _build_csv_data()
    lines = csv_text.split("\n")
    sum_a = 0.0
    sum_b = 0.0
    sum_c = 0.0
    max_a = -1.0
    max_b = -1.0
    max_c = -1.0
    count = 0
    # run 500 passes to bring timing into range
    pass_num = 0
    while pass_num < 500:
        sum_a = 0.0
        sum_b = 0.0
        sum_c = 0.0
        max_a = -1.0
        max_b = -1.0
        max_c = -1.0
        count = 0
        i = 1
        while i < len(lines):
            row = lines[i].split(",")
            a = float(row[1])
            b = float(row[2])
            c = float(row[3])
            sum_a = sum_a + a
            sum_b = sum_b + b
            sum_c = sum_c + c
            if a > max_a:
                max_a = a
            if b > max_b:
                max_b = b
            if c > max_c:
                max_c = c
            count = count + 1
            i = i + 1
        pass_num = pass_num + 1
    mean_a = sum_a / count
    mean_b = sum_b / count
    mean_c = sum_c / count
    return (
        int(mean_a * 100)
        + int(mean_b * 100)
        + int(mean_c * 100)
        + int(max_a)
        + int(max_b)
        + int(max_c)
    )


# ---------------------------------------------------------------------------
# 4. Path manipulation
#    Join, split, resolve 200K paths using string operations.
#    Tests: string ops, splitting, list building, object creation.
# ---------------------------------------------------------------------------


def path_manipulation():
    """Simulate pathlib-style path manipulation with string ops."""
    bases = ["/usr/local", "/home/user", "/var/log", "/tmp", "/opt/app"]
    components = ["bin", "lib", "share", "data", "config", "logs", "cache", "run"]
    extensions = [".py", ".txt", ".log", ".json", ".csv", ".cfg", ".dat", ".tmp"]
    results = []
    total_depth = 0
    i = 0
    while i < 200000:
        base = bases[i % len(bases)]
        mid = components[i % len(components)]
        ext = extensions[i % len(extensions)]
        # join
        path = base + "/" + mid + "/file_" + str(i) + ext
        # split into parts
        parts = path.split("/")
        total_depth = total_depth + len(parts)
        # extract parent and name
        name = parts[len(parts) - 1]
        parent = "/".join(parts[: len(parts) - 1])
        # extract extension
        dot_idx = name.rfind(".")
        if dot_idx >= 0:
            stem = name[:dot_idx]
            suffix = name[dot_idx:]
        else:
            stem = name
            suffix = ""
        # normalize: collapse double slashes
        cleaned = parent.replace("//", "/")
        results.append(len(cleaned) + len(stem) + len(suffix))
        i = i + 1
    return total_depth ^ sum(results) & 0xFFFFFF


# ---------------------------------------------------------------------------
# 5. Dataclass-style creation
#    Create 10K instances x 20 passes, serialize to dict, sort by field.
#    Tests: class instantiation, attribute access, sorting.
# ---------------------------------------------------------------------------


def dataclass_creation():
    """Create class instances, convert to dicts, sort."""

    class Record:
        def __init__(self, id, name, score, active):
            self.id = id
            self.name = name
            self.score = score
            self.active = active

        def to_dict(self):
            return {
                "id": self.id,
                "name": self.name,
                "score": self.score,
                "active": self.active,
            }

    checksum = 0
    pass_num = 0
    while pass_num < 20:
        records = []
        i = 0
        while i < 10000:
            r = Record(
                id=i,
                name="item_" + str((i * 7 + 13) % 9973),
                score=(i * 31 + 17) % 10000,
                active=i % 3 != 0,
            )
            records.append(r)
            i = i + 1
        # serialize all to dicts
        dicts = []
        j = 0
        while j < len(records):
            dicts.append(records[j].to_dict())
            j = j + 1
        # sort by score (use key extraction via list of tuples)
        scored = []
        k = 0
        while k < len(dicts):
            d = dicts[k]
            scored.append((d["score"], d["id"]))
            k = k + 1
        scored.sort()
        # checksum: sum of first 100 ids in sorted order
        m = 0
        while m < 100:
            checksum = checksum + scored[m][1]
            m = m + 1
        pass_num = pass_num + 1
    return checksum


# ---------------------------------------------------------------------------
# 6. Exception handling
#    Try/except in tight loop with 1% exception rate.
#    Tests: exception overhead, control flow.
# ---------------------------------------------------------------------------


def exception_handling():
    """Try/except with 1% exception rate in tight loop."""
    success_count = 0
    error_count = 0
    i = 0
    while i < 4000000:
        try:
            if i % 100 == 0:
                raise ValueError("expected failure")
            success_count = success_count + 1
        except ValueError:
            error_count = error_count + 1
        i = i + 1
    return success_count * 1000 + error_count


# ---------------------------------------------------------------------------
# 7. Generator pipeline
#    Chain 5 generators (filter, map, accumulate).
#    Tests: generator protocol, lazy evaluation, yield.
# ---------------------------------------------------------------------------


def generator_pipeline():
    """Chain generators: generate -> filter -> map -> accumulate -> collect."""

    def gen_range(n):
        """Generate integers 0..n."""
        i = 0
        while i < n:
            yield i
            i = i + 1

    def gen_filter_odd(source):
        """Pass through only odd values."""
        for x in source:
            if x % 2 == 1:
                yield x

    def gen_map_square(source):
        """Square each value."""
        for x in source:
            yield x * x

    def gen_accumulate(source):
        """Running sum."""
        total = 0
        for x in source:
            total = total + x
            yield total

    def gen_filter_threshold(source, threshold):
        """Pass through values above threshold."""
        for x in source:
            if x > threshold:
                yield x

    # run the pipeline multiple times to hit ~100ms
    checksum = 0
    pass_num = 0
    while pass_num < 30:
        pipeline = gen_filter_threshold(
            gen_accumulate(gen_map_square(gen_filter_odd(gen_range(50000)))), 1000000
        )
        count = 0
        last = 0
        for val in pipeline:
            count = count + 1
            last = val
        checksum = checksum + count + (last & 0xFFFF)
        pass_num = pass_num + 1
    return checksum


# ---------------------------------------------------------------------------
# 8. Dict merge + comprehension
#    Merge 100 dicts, build frequency table from 100K words.
#    Tests: dict operations, string hashing, iteration.
# ---------------------------------------------------------------------------


def dict_merge_and_freq():
    """Merge dicts and build word frequency table."""
    # build 100 dicts with overlapping keys
    dicts = []
    i = 0
    while i < 100:
        d = {}
        j = 0
        while j < 100:
            key = "key_" + str((i * 7 + j * 3) % 500)
            d[key] = i + j
            j = j + 1
        dicts.append(d)
        i = i + 1
    # merge all dicts (last writer wins)
    merged = {}
    di = 0
    while di < len(dicts):
        d = dicts[di]
        keys = list(d.keys())
        ki = 0
        while ki < len(keys):
            k = keys[ki]
            merged[k] = d[k]
            ki = ki + 1
        di = di + 1
    # build frequency table from 100K "words"
    vocab = [
        "alpha",
        "beta",
        "gamma",
        "delta",
        "epsilon",
        "zeta",
        "eta",
        "theta",
        "iota",
        "kappa",
        "lambda",
        "mu",
        "nu",
        "xi",
        "omicron",
        "pi",
        "rho",
        "sigma",
        "tau",
        "upsilon",
    ]
    total_top5 = 0
    pass_num = 0
    while pass_num < 15:
        freq = {}
        wi = 0
        while wi < 100000:
            word = vocab[wi % len(vocab)]
            if word in freq:
                freq[word] = freq[word] + 1
            else:
                freq[word] = 1
            wi = wi + 1
        # find top-5 by frequency (manual sort since no operator.itemgetter)
        pairs = []
        fkeys = list(freq.keys())
        fki = 0
        while fki < len(fkeys):
            k = fkeys[fki]
            pairs.append((freq[k], k))
            fki = fki + 1
        pairs.sort()
        pairs.reverse()
        top5_sum = 0
        ti = 0
        while ti < 5:
            top5_sum = top5_sum + pairs[ti][0]
            ti = ti + 1
        total_top5 = total_top5 + top5_sum
        pass_num = pass_num + 1
    return len(merged) * 1000 + total_top5


# ---------------------------------------------------------------------------
# 9. String formatting
#    f-string and .format() on 300K items.
#    Tests: string interpolation, type conversion, string building.
# ---------------------------------------------------------------------------


def string_formatting():
    """Format 300K strings using f-strings and .format()."""
    results_len = 0
    # f-string path
    i = 0
    while i < 150000:
        s = f"Item #{i}: value={i * 3.14:.2f}, hex=0x{i:08x}, name='obj_{i}'"
        results_len = results_len + len(s)
        i = i + 1
    # .format() path
    template = "Record {0}: score={1}, rank={2}, label={3}"
    j = 0
    while j < 150000:
        s = template.format(j, (j * 31) % 10000, j % 100, "L" + str(j % 26))
        results_len = results_len + len(s)
        j = j + 1
    return results_len


# ---------------------------------------------------------------------------
# 10. List comprehension chain
#     Nested iteration with filtering and function calls.
#     Tests: list building, function calls, filtering, nested iteration.
# ---------------------------------------------------------------------------


def list_comprehension_chain():
    """Chained list building with filtering and transforms."""

    def is_interesting(x):
        """Non-trivial predicate: digits sum to > 10."""
        total = 0
        n = x if x >= 0 else 0 - x
        while n > 0:
            total = total + n % 10
            n = n // 10
        return total > 10

    def transform(x):
        """Non-trivial transform."""
        return (x * x + 3 * x + 7) % 100003

    total = 0
    pass_num = 0
    while pass_num < 5:
        # stage 1: generate and filter
        data = []
        i = 0
        while i < 100000:
            if i % 3 != 0:
                data.append(i)
            i = i + 1
        # stage 2: transform
        transformed = []
        j = 0
        while j < len(data):
            transformed.append(transform(data[j]))
            j = j + 1
        # stage 3: filter interesting
        interesting = []
        k = 0
        while k < len(transformed):
            if is_interesting(transformed[k]):
                interesting.append(transformed[k])
            k = k + 1
        # stage 4: pair up adjacent and sum
        pairs = []
        m = 0
        while m < len(interesting) - 1:
            pairs.append(interesting[m] + interesting[m + 1])
            m = m + 1
        # stage 5: final filter and count
        big_count = 0
        p = 0
        while p < len(pairs):
            if pairs[p] > 50000:
                big_count = big_count + 1
            p = p + 1
        total = total + len(data) * 100 + len(interesting) * 10 + big_count
        pass_num = pass_num + 1
    return total


# ---------------------------------------------------------------------------
# Run all benchmarks
# ---------------------------------------------------------------------------

print("=" * 72)
print("Real-World Benchmark Suite")
print("=" * 72)

print("\n[1] JSON roundtrip (250 parse+modify+serialize cycles)")
bench("json_roundtrip", json_roundtrip)

print("\n[2] Regex matching (5 patterns x 10K strings x 15 passes)")
bench("regex_matching", regex_matching)

print("\n[3] CSV processing (1000 rows x 500 passes, column stats)")
bench("csv_processing", csv_processing)

print("\n[4] Path manipulation (200K paths, join/split/normalize)")
bench("path_manipulation", path_manipulation)

print("\n[5] Dataclass-style creation (200K instances, sort x 20)")
bench("dataclass_creation", dataclass_creation)

print("\n[6] Exception handling (4M iterations, 1% exception rate)")
bench("exception_handling", exception_handling)

print("\n[7] Generator pipeline (5-stage, 50K elements x 30 passes)")
bench("generator_pipeline", generator_pipeline)

print("\n[8] Dict merge + frequency table (100 dicts, 1.5M words)")
bench("dict_merge_and_freq", dict_merge_and_freq)

print("\n[9] String formatting (300K f-strings + .format())")
bench("string_formatting", string_formatting)

print("\n[10] List comprehension chain (100K x 5 passes)")
bench("list_comprehension_chain", list_comprehension_chain)

print("\n" + "=" * 72)
print("Done.")
print("=" * 72)
