# Minimized form of tests/differential/memory/alias_reassign_conditional_del.py,
# isolating the str(i % 7) corruption (the del/alias in that test were red
# herrings — the binding alias `y = x` is already refcount-balanced). The mod
# result feeds str(), an escape consumer that read the raw-carrier variable and
# saw box bits, corrupting the accumulated string.
def build(n):
    s = ""
    i = 0
    while i < n:
        s = s + str(i % 7)
        i = i + 1
    return s


print(build(20))
