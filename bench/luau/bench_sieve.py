def sieve(limit: int) -> int:
    is_prime: list[int] = []
    i: int = 0
    while i <= limit:
        is_prime.append(1)
        i = i + 1
    is_prime[0] = 0
    is_prime[1] = 0

    i = 2
    while i * i <= limit:
        if is_prime[i] == 1:
            j: int = i * i
            while j <= limit:
                is_prime[j] = 0
                j = j + i
        i = i + 1

    count: int = 0
    i = 0
    while i <= limit:
        count = count + is_prime[i]
        i = i + 1
    return count

def main() -> None:
    result: int = sieve(1000000)
    print(result)

main()
