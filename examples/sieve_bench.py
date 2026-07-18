def sieve(n: int) -> int:
    is_prime = [True] * (n + 1)
    is_prime[0] = is_prime[1] = False
    p = 2
    while p * p <= n:
        if is_prime[p]:
            for i in range(p * p, n + 1, p):
                is_prime[i] = False
        p += 1
    return sum(1 for value in is_prime if value)


count = sieve(10_000)
assert count == 1_229, f"Expected 1229 primes, got {count}"
print(f"Sieve(10000) = {count} primes")
