"""Purpose: code object metadata parity for core inspect/types fields."""

import types


def add(a, b=1):
    return a + b


co = add.__code__
print(isinstance(co, types.CodeType))
print(co.co_argcount, co.co_posonlyargcount, co.co_kwonlyargcount)
print(co.co_freevars, co.co_cellvars)
print(isinstance(co.co_flags, int))
print((co.co_flags & 0x03) == 0x03)


def read_before_store(flag):
    if flag:
        later
    early = 1
    later = 2
    return early + later


def evaluation_order():
    result = (inside := 3)
    return result + inside


def private_storage(source):
    result = [(found := item) for item in source]
    return found, result


def captured_storage(argument):
    local = argument

    def reader():
        return local

    other = 4
    return reader, other


def reducer_capture(argument):
    return sum(argument for item in (1, 2))


def inlined_capture(argument):
    readers = [lambda: argument for argument in (1, 2)]
    return argument, [reader() for reader in readers]


def default_private_storage(flag):
    if flag:
        factory = lambda values=[(found := item) for item in (1, 2)]: values
        assert factory() == [1, 2]
    return found


class Metadata:
    def method(self, flag):
        if flag:
            later
        early = 1
        later = 2
        return self, early, later

    async def coroutine(self, argument):
        local = argument
        return local

    def generator(self, argument):
        local = argument
        yield local


for function in (
    read_before_store,
    evaluation_order,
    private_storage,
    captured_storage,
    reducer_capture,
    inlined_capture,
    default_private_storage,
    Metadata.method,
    Metadata.coroutine,
    Metadata.generator,
):
    code = function.__code__
    print(function.__name__, code.co_varnames, code.co_cellvars, code.co_names)

print(read_before_store(False), evaluation_order(), private_storage([1, 2, 3]))
reader, other = captured_storage(7)
print(reader(), other)
print(reducer_capture(7), inlined_capture(9), default_private_storage(True))
try:
    default_private_storage(False)
except UnboundLocalError:
    print("default binding unbound")
else:
    raise AssertionError("an untaken default expression bound its walrus target")
