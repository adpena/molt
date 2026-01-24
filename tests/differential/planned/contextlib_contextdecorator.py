"""Purpose: check ContextDecorator usage on functions."""

import contextlib


class Decor(contextlib.ContextDecorator):
    def __enter__(self):
        print("enter")
        return self

    def __exit__(self, exc_type, exc, tb):
        print("exit")
        return False


@Decor()
def run():
    print("body")


run()
