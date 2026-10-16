"""super() resolves through a receiver class whose __bases__ changes mid-lookup."""

# MOLT_META: verified_subset_scope=dynamic_execution_policy expect_fail=molt expect_fail_reason=too_dynamic_policy


class Root:
    pass


class Expected(Root):
    pass


class After(Root):
    pass


class Before(Root):
    def __getattribute__(self, name):
        if name == "__class__":
            object.__setattr__(self, "__class__", After)
            # Reassigning a finished class's bases is outside Molt's verified
            # subset (spec 0216): compiled layouts and method resolution are
            # fixed at class creation.
            Before.__bases__ = (Expected,)
            return Before
        return object.__getattribute__(self, name)


receiver = Before()
try:
    result = super(Expected, receiver)
    outcome = ("resolved", result.__self_class__.__name__)
except BaseException as error:
    outcome = (type(error).__name__, str(error))
print(outcome, type(receiver).__name__, issubclass(Before, Expected))
