# RC drop-insertion per-site verification (design 20 §4.1): CHECK_EXCEPTION
# observation site.
#
# A loop whose body performs operations that are observation-only potentially-
# throwing ops (int() parse, dict/index access) — each followed by a universal
# CheckException (C2). The function has NO try/except handler, so the drop pass
# still processes it (has_exception_handlers() is false). Verifies that the
# native value-tracking RC suppression does not leak across the check_exception
# boundaries (where drain_cleanup_tracked_dedup formerly fired) and that the
# heap temporaries created between checks are freed by the TIR drops. A
# per-iteration leak across the check boundary grows RSS without bound.
def parse_sum(n):
    total = 0
    i = 0
    while i < n:
        s = str(i)            # heap string temp; CheckException follows
        v = int(s)            # parse back; CheckException follows
        total = total + v
        d = {"k": s}          # heap dict + index; CheckException follows
        total = total + len(d["k"])
        i = i + 1
    return total


print(parse_sum(100000))
