"""Source-frame super and PEP 709/generator distinction across execution targets."""
# MOLT_META: min_py=3.12

import asyncio


class Base:
    def value(self):
        return 7


class Subject(Base):
    def materialized(self):
        return (
            [super().value() for item in (1, 2)],
            sorted({super().value() for item in (1, 2)}),
            {item: super().value() for item in (1, 2)},
            [super().value() for item in (1,) for inner in (2,)],
            [[super().value() for inner in (1,)] for item in (2,)],
            [super().value() for (item, *rest) in ((1, 2, 3),)],
        )

    def generator(self):
        return list(super().value() for item in (1,))

    def explicit_generator(self):
        return list(super(Subject, self).value() for item in (1,))

    def outer_iterable(self):
        return list(item for item in (super().value(),))

    def generator_sum(self):
        return sum(super().value() for item in range(2))

    def generator_any(self):
        return any(super().value() for item in range(2))

    def generator_all(self):
        return all(super().value() for item in range(2))

    def list_sum(self):
        return sum([super().value() for item in range(2)])

    def shadowed_argument(self):
        return [super().value() for self in (0,)]

    def restored_after_exception(self):
        try:
            values = [super().value() for self in (0,)]
        except TypeError:
            return super().value()
        return "missed"

    def nested_function(self):
        def inner(receiver):
            return super().value()

        return inner(self)

    def nested_without_arguments(self):
        return (lambda: super().value())()

    def variadic_only(*args):
        return super().value()

    def keyword_only(*, self):
        return super().value()

    def no_cell(self):
        __class__ = Subject
        return super().value()

    async def asynchronous(self):
        async def values():
            yield 1
            yield 2

        return [super().value() async for item in values()]


def show(label, function):
    try:
        print(label, function())
    except Exception as error:
        print(label, type(error).__name__, str(error))


subject = Subject()
show("materialized", subject.materialized)
show("generator", subject.generator)
show("explicit-generator", subject.explicit_generator)
show("outer-iterable", subject.outer_iterable)
show("sum-generator", subject.generator_sum)
show("any-generator", subject.generator_any)
show("all-generator", subject.generator_all)
show("sum-list", subject.list_sum)
show("shadowed-arg", subject.shadowed_argument)
show("restored", subject.restored_after_exception)
show("nested-function", subject.nested_function)
show("nested-no-args", subject.nested_without_arguments)
show("variadic-only", subject.variadic_only)
show("keyword-only", lambda: Subject.keyword_only(self=subject))
show("local-class", subject.no_cell)
show("async-comprehension", lambda: asyncio.run(subject.asynchronous()))
