"""Purpose: differential coverage for collections.abc awaitable/coroutine/buffer."""

import collections.abc as abc


class AwaitableOnly:
    def __await__(self):
        if False:
            yield None
        return 123


class CoroutineOnly:
    def __await__(self):
        if False:
            yield None
        return 0

    def send(self, _value):
        return None

    def throw(self, _typ, _val=None, _tb=None):
        return None

    def close(self):
        return None


print(issubclass(AwaitableOnly, abc.Awaitable), isinstance(AwaitableOnly(), abc.Awaitable))
print(issubclass(CoroutineOnly, abc.Coroutine), isinstance(CoroutineOnly(), abc.Coroutine))
print(issubclass(bytes, abc.Buffer), isinstance(b"hi", abc.Buffer))
print(issubclass(bytearray, abc.Buffer), isinstance(bytearray(b"hi"), abc.Buffer))
print(isinstance(memoryview(b"hi"), abc.Buffer))
