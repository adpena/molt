"""Runtime-like names must preserve Python callable lookup and evaluation order."""

events = []


def argument(label, value):
    events.append(label)
    return value


class Recorder:
    def __call__(self, *args, **kwargs):
        events.append("type-call")
        return args, kwargs


def ordinary(**kwargs):
    events.append("ordinary")
    return kwargs


molt_spawn = Recorder()
# Special-method lookup uses the type, not the instance dictionary.
molt_spawn.__call__ = lambda *args, **kwargs: "wrong-instance-call"
print(molt_spawn(argument("positional", 1), keyword=argument("keyword", 2)))
print(events)

molt_chan_send = Recorder()
molt_chan_recv = ordinary
print(molt_chan_send(python_argument=3))
print(molt_chan_recv(python_argument=4))
# A live rebinding cannot inherit admission or signature from its old value.
molt_chan_send = ordinary
print(molt_chan_send(python_argument=5))


def local_calls(molt_async_sleep, molt_cancel_current):
    print(molt_async_sleep(result=argument("local-sleep", 6)))
    return molt_cancel_current(result=argument("local-cancel", 7))


print(local_calls(Recorder(), ordinary))


class Failing:
    def __call__(self, *args, **kwargs):
        events.append("failing-call")
        raise ValueError("call body")


molt_block_on = Failing()
try:
    molt_block_on(argument("before-error", 8), detail=argument("error-keyword", 9))
except ValueError as exc:
    print(type(exc).__name__, str(exc))

molt_promise_new = 0
try:
    molt_promise_new(argument("noncallable-argument", 10))
except TypeError as exc:
    print(type(exc).__name__, str(exc))


async def async_calls(molt_chan_send, molt_chan_recv):
    print(molt_chan_send(value=argument("async-send", 11)))
    return molt_chan_recv(value=argument("async-recv", 12))


# No await exists: both ordinary calls complete in the first resume.
coroutine = async_calls(Recorder(), ordinary)
try:
    coroutine.send(None)
except StopIteration as exc:
    print("async-return", exc.value)
print(events)
