# MOLT_ENV: MOLT_CODEC=json


def add13(a1, a2, a3, a4, a5, a6, a7, a8, a9, a10, a11, a12, a13):
    return a1 + a2 + a3 + a4 + a5 + a6 + a7 + a8 + a9 + a10 + a11 + a12 + a13


def main():
    args = (1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13)
    print(add13(1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13))
    fn = add13
    print(fn(*args))


if __name__ == "__main__":
    main()
