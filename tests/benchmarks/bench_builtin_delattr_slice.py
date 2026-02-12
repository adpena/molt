class Box:
    pass


def main() -> None:
    call_delattr = delattr
    i = 0
    total = 0
    while i < 150_000:
        obj = Box()
        obj.value = i
        call_delattr(obj, "value")
        if not hasattr(obj, "value"):
            total += 1
        i += 1
    print(total)


if __name__ == "__main__":
    main()
