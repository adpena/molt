from molt.shims import molt_chan_drop, molt_chan_new


def main() -> None:
    total = 0
    i = 0
    while i < 100_000:
        handle = molt_chan_new(0)
        molt_chan_drop(handle)
        total += 1
        i += 1

    print(total)


if __name__ == "__main__":
    main()
