# Resource enforcement scenario: rapid allocation must be stopped.
# Expected: allocation limit exceeded when max_allocations=1000.
objects = []
for i in range(10_000):
    objects.append([i])
