"""Purpose: differential coverage for this module globals and ROT13 mapping."""

import this


def main() -> None:
    print("s_len", len(this.s))
    print("globals", this.c, this.i)
    print("mapping", this.d["A"], this.d["n"])
    decoded = "".join(this.d.get(ch, ch) for ch in this.s)
    print("title", decoded.splitlines()[0])
    print("suffix", decoded.endswith("those!"))


if __name__ == "__main__":
    main()
