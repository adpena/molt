# Differential: int(s.split(sep)[k]) direct-parse fusion + the in-line
# split-field deforestation (len/ord/eq against bounds). Every case here must be
# byte-identical to CPython whether or not the split-field optimizations fire —
# they are pure perf rewrites of the materializing path.


def parse_int(text: str) -> int:
    # Hand-rolled decimal parser — the shape the inliner splices so the
    # deforestation pass rewrites its len(text)/ord(text[i]) against the field
    # bounds (no per-field heap string).
    value = 0
    i = 0
    while i < len(text):
        value = value * 10 + (ord(text[i]) - 48)
        i += 1
    return value


def main() -> None:
    # --- int(s.split(sep)[k]) direct fusion, the common real-world idiom -------
    line = "12|0034|-56|7890123456789012345678901234567890|"
    print(int(line.split("|")[0]))  # plain
    print(int(line.split("|")[1]))  # leading zeros
    print(int(line.split("|")[2]))  # negative
    print(int(line.split("|")[3]))  # bigint (overflows i64, must stay exact)

    # invalid-literal ValueError (message must match CPython exactly)
    try:
        print(int("ab|cd".split("|")[0]))
    except ValueError as e:
        print("ValueError:", e)
    # empty field -> ValueError
    try:
        print(int(line.split("|")[4]))
    except ValueError as e:
        print("ValueError:", e)
    # out-of-range field index -> IndexError
    try:
        print(int(line.split("|")[99]))
    except IndexError as e:
        print("IndexError:", e)

    # explicit base must NOT take the base-10 fast path (stays materializing).
    print(int("ff|10".split("|")[0], 16))
    print(int("0b101|9".split("|")[0], 2))

    # --- hand-rolled parse_int over split fields (deforestation target) --------
    data = "100|205|3|0|42|999999"
    total = 0
    k = 0
    while k < 6:
        total = total + parse_int(data.split("|")[k])
        k += 1
    print(total)

    # --- len / ord / == consumption matrix on a split field -------------------
    rec = "alpha|bravo|charlie"
    f0 = rec.split("|")[0]
    print(len(f0))  # len(field)
    print(ord(f0[0]), ord(f0[-1]))  # ord(field[i]) incl negative index
    print(rec.split("|")[1] == "bravo")  # field == const
    print(rec.split("|")[2] == "delta")

    # --- a field that ESCAPES must keep materializing (still byte-identical) ---
    parts = "x|y|z".split("|")
    held = parts[1]
    print(held, held.upper(), len(held))

    # --- multi-char separator + the field flowing to parse_int ----------------
    mc = "11::22::33"
    print(parse_int(mc.split("::")[0]) + parse_int(mc.split("::")[2]))

    # --- non-ASCII field: len()/ord() must be codepoint-correct ---------------
    uni = "café|über|naïve"
    uf = uni.split("|")[0]
    print(len(uf), ord(uf[3]))
    print(uni.split("|")[1] == "über")


if __name__ == "__main__":
    main()
