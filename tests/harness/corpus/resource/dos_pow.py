# Resource enforcement scenario: huge exponentiation must be rejected.
# Expected: MemoryError from pre-emptive DoS guard.
result = 2 ** 10_000_000
