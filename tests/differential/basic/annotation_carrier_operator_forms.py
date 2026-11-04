events = []
class Owner:
    def __init__(self, label):
        self.label = label
    def __add__(self, other):
        return self
    def __lt__(self, other):
        return self
    def __bool__(self):
        return True
    def __del__(self):
        events.append(self.label)

def add(x: float, y: float) -> float:
    return x + y
def select(x: bool, y: bool) -> bool:
    return x and y
def compare(x: int, y: int) -> bool:
    return x < y
def loop(x: float) -> float:
    result: float = x
    for i in range(3):
        result = result + x
    return result
def handler(x: bool) -> bool:
    result: bool = True
    try:
        raise ValueError('test')
    except ValueError:
        result = x
    return result

def exercise():
    for kind in range(5):
        owner = Owner(kind)
        if kind == 0:
            result = add(owner, owner)
        elif kind == 1:
            result = select(owner, owner)
        elif kind == 2:
            result = compare(owner, owner)
        elif kind == 3:
            result = loop(owner)
        else:
            result = handler(owner)
        print(kind, result is owner, type(result).__name__)
        del owner
        print('held', events)
        del result
        print('released', events)
exercise()

def failing(x: float) -> bool:
    alias = x
    raise ValueError('failed body')

def errors():
    owner = Owner('error')
    try:
        failing(owner)
    except ValueError:
        pass
    del owner
    print('body-error', events)
    owner = Owner('binding-error')
    try:
        failing()
    except TypeError:
        pass
    del owner
    print('binding-error', events)
errors()
