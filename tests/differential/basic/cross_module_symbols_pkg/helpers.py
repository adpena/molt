"""Module functions a caller in another module calls by attribute.

`main` compiles under a renamed symbol; `molt_user_main` would collide with
it; `step` and `step_poll` share a stateful companion name.
"""


def main() -> str:
    return "main"


def molt_user_main() -> str:
    return "molt_user_main"


def double(value: int) -> int:
    return value * 2


def step(value: int) -> int:
    return value + 1


def step_poll(value: int) -> int:
    return value - 1
