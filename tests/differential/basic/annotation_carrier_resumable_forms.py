events = []

class Owner:
    def __init__(self, label):
        self.label = label
    def __del__(self):
        events.append(self.label)

def generator(x: float) -> bool:
    try:
        sent: bool = yield x
        yield sent
    finally:
        events.append('finally')

async def coroutine(x: bool) -> float:
    return x

async def nested_coroutine(x: float) -> bool:
    return await coroutine(x)

def exercise():
    owner = Owner('generator')
    g = generator(owner)
    a = next(g)
    print('yield', a is owner, type(a).__name__)
    b = g.send(owner)
    print('send', b is owner, type(b).__name__)
    g.close()
    del g, owner
    print('held', events)
    del a, b
    print('released', events)
    for function in (coroutine, nested_coroutine):
        owner = Owner('coroutine')
        c = function(owner)
        try:
            c.send(None)
        except StopIteration as error:
            a = error.value
        print('await', a is owner, type(a).__name__)
        del c, owner
        print('held', events)
        del a
        print('released', events)
exercise()
