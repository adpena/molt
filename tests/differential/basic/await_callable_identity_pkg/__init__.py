"""A foreign callable whose import alias deliberately resembles a builtin."""


async def completed(*args, **kwargs):
    return args, kwargs
