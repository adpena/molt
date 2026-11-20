"""Purpose: a thrown exception class is constructed while its target runs (3.15).

CPython 3.15 normalizes the argument of throw() with the generator or
coroutine already executing, so a constructor that inspects or resumes it
sees gi_running/cr_running true and "already executing"; 3.12 to 3.14
construct it first. A constructor failure is thrown into the frame instead.
"""

import types

events = []


class Observing(type):
    def __call__(cls, *args):
        target = cls.target
        running = getattr(target, "gi_running", None)
        if running is None:
            running = target.cr_running
        try:
            if isinstance(target, types.GeneratorType):
                outcome = ("send", target.send(None))
            else:
                outcome = ("no-send",)
        except BaseException as error:
            outcome = ("send-error", type(error).__name__, str(error))
        events.append((cls.__name__, running, outcome))
        return type.__call__(cls, *args)


class Failing(type):
    def __call__(cls, *args):
        raise ValueError("constructor failed")


class Observed(Exception, metaclass=Observing):
    target = None


class Broken(Exception, metaclass=Failing):
    pass


def gen():
    try:
        yield "first"
        yield "second"
    except BaseException as error:
        yield ("caught", type(error).__name__)


suspended = gen()
next(suspended)
Observed.target = suspended
print("suspended", suspended.throw(Observed), events[-1])

created = gen()
Observed.target = created
try:
    print("created-result", created.throw(Observed), events[-1])
except Observed:
    print("created", events[-1], created.gi_frame is None)

failing = gen()
next(failing)
print("constructor-failure", failing.throw(Broken))

try:
    gen().throw(Broken)
except ValueError as error:
    print("created-constructor-failure", error)


async def coroutine():
    return 1


coro = coroutine()
Observed.target = coro
try:
    coro.throw(Observed)
except Observed:
    print("coroutine-created", events[-1][:2])
