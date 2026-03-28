# Resource enforcement scenario: huge string repetition must be rejected.
# Expected: MemoryError from pre-emptive DoS guard.
result = "x" * 10_000_000_000
