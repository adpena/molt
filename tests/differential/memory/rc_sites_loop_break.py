# RC drop-insertion per-site verification (design 20 §4.1): LOOP BREAK site.
#
# A heap accumulator built in a loop that exits via `break` (not the loop
# condition). The loop-carried value is live across the break edge; verifies the
# native value-tracking RC suppression (no drain at the break boundary) does not
# leak the carried value, and the post-loop use + final drop are balanced. Each
# repetition resets the accumulator so a per-iteration leak would grow RSS.
def concat_break(limit):
    s = ""
    i = 0
    while True:
        s = s + "z"
        i = i + 1
        if i >= limit:
            break           # break edge — carried s live to the post-loop use
    return len(s)


def driver(reps):
    total = 0
    j = 0
    while j < reps:
        total = total + concat_break(50)
        j = j + 1
    return total


print(driver(20000))
