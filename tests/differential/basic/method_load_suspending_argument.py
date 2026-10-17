"""Purpose: a method call whose argument suspends keeps CPython's order (HF-216).

In a generator and a coroutine the method is loaded before the suspension, and
an argument that rebinds the method while suspended does not change the call.
"""

import asyncio


class Sink:
    def __init__(self):
        self.seen = []

    def put(self, *values):
        self.seen.append(values)
        return len(self.seen)


def generator(sink):
    count = sink.put((yield "first"), (yield "second"))
    yield count


sink = Sink()
gen = generator(sink)
print(next(gen))
print(gen.send("a"))
Sink.put = lambda self, *values: "rebound"
print(gen.send("b"))
print(sink.seen)
del Sink.put


class Recorder:
    def __init__(self):
        self.seen = []

    def record(self, value, other):
        self.seen.append((value, other))
        return value + other


async def produce(value):
    await asyncio.sleep(0)
    return value


async def main():
    recorder = Recorder()
    first = recorder.record(await produce(1), await produce(2))
    second = recorder.record(3, await produce(4))
    return first, second, recorder.seen


print(asyncio.run(main()))


# CPython owns the loaded callable/receiver before evaluating an argument.
# An argument failure unwinds these owners before the caller's except body.
events = []


class LifetimeReceiver:
    def __init__(self):
        events.append("receiver-new")

    def __del__(self):
        events.append("receiver-drop")

    def invoke(self, *args, **kwargs):
        events.append("invoke")
        return "called"


class Argument:
    def __init__(self, label="argument"):
        self.label = label
        events.append(label + "-new")

    def __del__(self):
        # A nested handled error in a finalizer must not replace the exception
        # that triggered the caller's operand cleanup.
        try:
            raise LookupError("cleanup")
        except LookupError:
            events.append(self.label + "-drop")


def fail_argument():
    events.append("argument-fail")
    raise ValueError("argument")


try:
    LifetimeReceiver().invoke(value=fail_argument())
except ValueError:
    events.append("caught")
print("keyword-unwind", events)
events.clear()


def suspended_failure():
    try:
        LifetimeReceiver().invoke((yield "positional"), value=(yield "keyword"))
    except ValueError:
        events.append("caught")
    yield "done"


pending = suspended_failure()
print("suspended-unwind", next(pending))
print("suspended-unwind", pending.send(Argument()))
print("suspended-unwind", pending.throw(ValueError("injected")))
print("suspended-unwind", events)
pending.close()
events.clear()


# A descriptor may return an unrelated callable, so LOAD_METHOD's receiver
# word is null and the original temporary receiver can die before arguments.
class CallableValue:
    def __del__(self):
        events.append("callable-drop")

    def __call__(self, *args, **kwargs):
        events.append("callable-call")


class CallableDescriptor:
    def __get__(self, instance, owner):
        events.append("descriptor-get")
        return CallableValue()


class DescriptorReceiver:
    invoke = CallableDescriptor()

    def __del__(self):
        events.append("descriptor-receiver-drop")


try:
    DescriptorReceiver().invoke(value=fail_argument())
except ValueError:
    events.append("caught")
print("null-keyword-unwind", events)
events.clear()


def suspended_descriptor_failure():
    try:
        DescriptorReceiver().invoke((yield "positional"), value=(yield "keyword"))
    except ValueError:
        events.append("caught")
    yield "done"


pending = suspended_descriptor_failure()
print("null-suspended-unwind", next(pending))
print("null-suspended-unwind", pending.send(Argument()))
print("null-suspended-unwind", pending.throw(ValueError("injected")))
print("null-suspended-unwind", events)
pending.close()


events.clear()


def suspended_keyword_group_failure():
    try:
        LifetimeReceiver().invoke(
            one=(yield "one"), two=(yield "two"), three=(yield "three")
        )
    except ValueError as error:
        events.append("caught:" + str(error))
    yield "done"


pending = suspended_keyword_group_failure()
print("keyword-group-unwind", next(pending))
print("keyword-group-unwind", pending.send(Argument("one")))
print("keyword-group-unwind", pending.send(Argument("two")))
print("keyword-group-unwind", pending.throw(ValueError("injected-keywords")))
print("keyword-group-unwind", events)
pending.close()
