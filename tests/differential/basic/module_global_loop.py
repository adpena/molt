# Module-level while loop over module-global variables — the shape the TIR
# module-slot-promotion pass rewrites (globals carried as SSA loop phis, stored
# back at exits). Values must be byte-identical to CPython.
N = 1000
total = 0
i = 0
while i < N:
    total = total + i
    i = i + 1
print(total, i, N)
total = total * 2
print(total)
