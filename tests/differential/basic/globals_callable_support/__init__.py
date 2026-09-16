"""Cross-module acquisition and invocation for the globals callable capsule."""

alias = globals


def invoke(fn):
    return fn()
