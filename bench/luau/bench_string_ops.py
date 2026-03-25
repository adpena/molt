def main() -> None:
    result: str = ""
    i: int = 0
    while i < 10000:
        result = result + str(i) + " "
        i = i + 1

    count: int = 0
    in_word: int = 0
    j: int = 0
    while j < len(result):
        c: str = result[j]
        if c == " ":
            if in_word == 1:
                count = count + 1
                in_word = 0
        else:
            in_word = 1
        j = j + 1
    if in_word == 1:
        count = count + 1

    print(count)
    print(len(result))

main()
