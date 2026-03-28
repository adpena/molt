# Resource enforcement scenario: allocation loop must be stopped by memory limit.
# Expected: MemoryError (uncatchable) when max_memory=1MB.
data = []
while True:
    data.append(b"x" * 1024)
