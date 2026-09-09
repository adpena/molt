"""Versioned evaluator argument zero, class cells, and source-name custody."""

import sys


def outcome(label, thunk):
    try:
        print(label, thunk())
    except Exception as error:
        print(label, type(error).__name__, str(error))


format = "module"
type ModuleFormat = format
type ModuleSuper = (format, super())

print("module-format", ModuleFormat.__value__)
outcome("module-alias", lambda: ModuleSuper.__value__)


class Owner:
    format = "class"
    type Format = format
    type Alias = (format, super())

    def bounded[T: super()]():
        pass

    def constrained[T: (super(), int)]():
        pass


print("class-format", Owner.Format.__value__)
outcome("class-alias", lambda: Owner.Alias.__value__)
outcome("class-bound", lambda: Owner.bounded.__type_params__[0].__bound__)
outcome(
    "class-constraints", lambda: Owner.constrained.__type_params__[0].__constraints__
)


def closure_types(format):
    __class__ = int
    type Format = format
    type Alias = (format, super().__self__)
    type Shadow = [super().__self__ for format in (0,)]
    return Format, Alias, Shadow


ClosureFormat, ClosureAlias, ClosureShadow = closure_types("closure")
print("closure-format", ClosureFormat.__value__)
outcome("closure-alias", lambda: ClosureAlias.__value__)
outcome("closure-comprehension", lambda: ClosureShadow.__value__)

if sys.version_info >= (3, 14):

    def ordinary(value: format):
        pass

    print("ordinary-module", ordinary.__annotations__)

    def closure_annotations(format):
        __class__ = int

        def value(item: (format, super().__self__)):
            pass

        def shadow(item: [super().__self__ for format in (0,)]):
            pass

        return value, shadow

    closure_value, closure_shadow = closure_annotations("closure")
    print("ordinary-closure", closure_value.__annotations__)
    print("ordinary-comprehension", closure_shadow.__annotations__)

    class AnnotatedOwner:
        format = "class"
        value: (format, super())

        def method(value: (format, super())):
            pass

    outcome("ordinary-class", lambda: AnnotatedOwner.__annotations__)
    outcome("ordinary-method", lambda: AnnotatedOwner.method.__annotations__)

    # Direct public evaluator calls accept arbitrary objects. Their one rich
    # comparison must return the singleton False; no __bool__ callback is made.
    for requested in (0, -1, False, True, 1.0, 2, 3, "1"):
        outcome(
            "ordinary-format-" + repr(requested),
            lambda: ordinary.__annotate__(requested),
        )
        outcome(
            "alias-format-" + repr(requested),
            lambda: ModuleFormat.evaluate_value(requested),
        )

    format_events = []

    class FormatTruth:
        def __bool__(self):
            format_events.append("bool")
            return False

    class FormatValue:
        def __gt__(self, limit):
            format_events.append(("gt", limit))
            return FormatTruth()

        def __eq__(self, other):
            raise AssertionError("format protocol must not compare equality")

    for evaluator in (ordinary.__annotate__, ModuleFormat.evaluate_value):
        format_events.clear()
        outcome("custom-format", lambda: evaluator(FormatValue()))
        print("format-events", format_events)

    class ScalarFormatValue:
        def __init__(self, result):
            self.result = result

        def __gt__(self, limit):
            return self.result

    for result in (False, True, 0, None, []):
        for evaluator in (ordinary.__annotate__, ModuleFormat.evaluate_value):
            outcome(
                "comparison-result-" + repr(result),
                lambda: evaluator(ScalarFormatValue(result)),
            )
