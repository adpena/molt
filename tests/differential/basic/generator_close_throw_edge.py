"""Purpose: differential coverage for generator close/throw edge cases."""


def gen():
    try:
        yield "start"
        yield "middle"
    finally:
        yield "cleanup"


if __name__ == "__main__":
    iterator = gen()
    print("first", next(iterator))
    try:
        iterator.close()
        print("close", "done")
    except Exception as exc:
        print("close", type(exc).__name__)

    iterator2 = gen()
    print("first2", next(iterator2))
    try:
        iterator2.throw(ValueError("boom"))
        print("throw", "done")
    except Exception as exc:
        print("throw", type(exc).__name__)

    events = []

    class Captured:
        def __init__(self, label):
            self.label = label

        def __del__(self):
            events.append(self.label)

    def never_started(capture):
        yield capture

    # A throw into a created generator raises at its first instruction: the
    # traceback's frame keeps the argument alive until the exception dies.
    created = never_started(Captured("thrown-capture"))
    failure = LookupError("before-start")
    try:
        created.throw(failure)
    except LookupError as error:
        print("thrown-created", error is failure, events, created.gi_frame is None)
        caught = error
    print("thrown-created-held", events)
    del caught, failure
    print("thrown-created-released", events)
