events = []

class Owner:
    def __neg__(self):
        return self
    def __pos__(self):
        return self
    def __del__(self):
        events.append('released')

def negative(x: float) -> float:
    return -x

def positive(x: float) -> float:
    return +x

def logical(x: bool) -> bool:
    return not x

for function in (negative, positive):
    events.clear()
    owner = Owner()
    alias = function(owner)
    print(function.__name__, alias is owner, type(alias).__name__)
    del owner
    print('held', events)
    del alias
    print('released', events)

print('not-two', logical(2))
