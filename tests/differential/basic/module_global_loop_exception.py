# Exception raised MID-LOOP by an arithmetic op inside a promoted module-global
# loop: the handler (and everything after) must observe exactly the values an
# unpromoted per-iteration-store loop would have left in the module dict — the
# compensation-store path of the module-slot-promotion pass.
total = 0
i = 0
try:
    while i < 10:
        total = total + 100 // (5 - i)  # ZeroDivisionError at i == 5
        i = i + 1
except ZeroDivisionError:
    print("caught", total, i)
print(total, i)
