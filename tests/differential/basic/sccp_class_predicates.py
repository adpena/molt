# Identity and equality decided by exact builtin classes: a wrong class fold
# changes these results. Each value below is computed, not a literal.


def classify(xs, word):
    n = len(xs)
    flag = n > 1
    half = n / 2
    text = word + "!"
    results = []
    if n is None:
        results.append("int is None")
    else:
        results.append("int is not None")
    if flag is not None:
        results.append("bool is not None")
    if flag == "True":
        results.append("bool equals str")
    if text != None:  # noqa: E711 - the comparison under test
        results.append("str differs from None")
    if half == n:
        results.append("float equals int")
    if flag == 1:
        results.append("bool equals one")
    if flag is True:
        results.append("flag is True")
    return results


def none_paths(value):
    marker = None if value else value
    return marker is None, marker == None  # noqa: E711


for xs, word in (([], "a"), ([1, 2], "bb"), ([1, 2, 3], "")):
    print(classify(xs, word))
print(none_paths(1), none_paths(0), none_paths(""))
