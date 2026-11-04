events = []
class Owner:
    def __init__(self, label):
        self.label = label
    def __del__(self):
        events.append(self.label)

def echo_none(x: None) -> None:
    return x

def make_none() -> None:
    return Owner('created')

def use():
    owner = Owner('borrowed')
    a = echo_none(owner)
    print('identity', a is owner, type(a).__name__)
    del owner
    print('held', events)
    del a
    print('released', events)
    make_none()
    print('ignored', events)
    owner = Owner('ignored-echo')
    echo_none(owner)
    del owner
    print('echo', events)
use()
