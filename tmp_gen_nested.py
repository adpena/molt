def leaf():
    yield 1


def gen():
    for x in leaf():
        yield x


print("next gen", next(gen()))

it = gen()
for i in range(3):
    try:
        print("step", i, next(it))
    except StopIteration:
        print("stop", i)
        break
