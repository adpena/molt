# Direct SSA reads across joins, exception transfers and generator resumes.
def exercise(limit, fail_at):
    total = 0
    events = []
    for i in range(limit):
        try:
            if i == fail_at:
                raise ValueError(i)
            if i % 2:
                step = i * 3
            else:
                step = i + 2
            if i == 2:
                continue
            total += step
            if total > 40:
                break
        except ValueError as error:
            events.append(error.args[0])
            total -= i
        finally:
            events.append(total)
    return total, events


for size in (0, 1, 4, 10):
    for failure in (-1, 0, 3, 8):
        print(size, failure, exercise(size, failure))


def resume():
    value = 1
    for i in range(4):
        try:
            value += yield value
        except ValueError:
            value = -value
        finally:
            value += i


iterator = resume()
print(next(iterator))
print(iterator.send(5))
print(iterator.throw(ValueError()))
print(iterator.send(2))
try:
    iterator.send(7)
except StopIteration:
    print("done")
