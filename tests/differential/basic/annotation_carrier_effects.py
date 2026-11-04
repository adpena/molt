events = []


class Owner:
    def __add__(self, other):
        events.append('add')
        raise ValueError('operator')


def unused(x: int, y: int):
    x + y


def handled(x: int, y: int):
    try:
        x + y
    except ValueError as error:
        events.append(str(error))


owner = Owner()
try:
    unused(owner, owner)
except ValueError as error:
    print('unused', str(error))
handled(owner, owner)
print(events)

class Truth:
    def __bool__(self):
        events.append('truth')
        raise ValueError('truth-body')


def unused_not(value: bool):
    not value


def caught_branch(value: bool):
    try:
        if value:
            events.append('unexpected')
    except ValueError as error:
        events.append(str(error))


value = Truth()
try:
    unused_not(value)
except ValueError as error:
    print('unused-not', str(error))
caught_branch(value)
print(events)
