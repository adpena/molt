"""Purpose: callable default arguments should preserve the captured callable."""


def identity(value):
    return value


def call_default(value, fn=identity):
    return fn(value)


print(call_default("ok"))
