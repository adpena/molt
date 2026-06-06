# RC drop-insertion per-site verification (design 20 §4.1): CONDITIONAL JOIN site.
#
# A heap value produced on BOTH arms of an if/else inside a loop, joined at the
# merge point (a phi). Verifies the native value-tracking RC suppression handles
# the join-slot correctly: the value flowing out of each arm is owned, the merge
# carries exactly one owned reference, and the loop-carried drop releases the
# previous iteration's join result exactly once on each path (no leak, no
# double-free of the path-divergent temporary).
def branchy(n):
    s = ""
    i = 0
    while i < n:
        if i % 2 == 0:
            s = s + "even"     # arm A produces a new string
        else:
            s = s + "od"       # arm B produces a new string
        i = i + 1
    return len(s)


def driver(reps):
    total = 0
    j = 0
    while j < reps:
        total = total + branchy(60)
        j = j + 1
    return total


print(driver(20000))
