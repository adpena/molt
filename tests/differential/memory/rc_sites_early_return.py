# RC drop-insertion per-site verification (design 20 §4.1): EARLY RETURN site.
#
# A heap accumulator built in a loop, returned early (mid-loop) on a condition.
# Verifies the native value-tracking RC suppression for drop-inserted functions
# does not leak the in-flight temporaries on the early-return path nor
# double-free the returned value (the return ABI transfers ownership; the TIR
# drops release everything dead at the early-return point). Each call frees all
# but the returned string.
def build_until(n, stop):
    s = ""
    i = 0
    while i < n:
        s = s + "ab"
        if i == stop:
            return s        # early return — s transfers to caller, rest is dead
        i = i + 1
    return s


def driver(reps):
    total = 0
    j = 0
    while j < reps:
        r = build_until(100, 5)
        total = total + len(r)
        j = j + 1
    return total


print(driver(20000))
