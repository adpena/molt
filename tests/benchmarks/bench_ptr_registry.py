from molt.shims import molt_chan_drop, molt_chan_new

total = 0
i = 0
while i < 200_000:
    handle = molt_chan_new(0)
    molt_chan_drop(handle)
    total += 1
    i += 1

print(total)
