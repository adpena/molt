def run_case(mode):
    class Root:
        pass

    class Expected(Root):
        pass

    class After(Expected if mode == "returns_new_type" else Root):
        pass

    class AfterRelease(Root):
        pass

    class RejectedClaim:
        def __init__(self, receiver):
            self.receiver = receiver

        def __del__(self):
            object.__setattr__(self.receiver, "__class__", AfterRelease)

    class Before(Root):
        def __getattribute__(self, name):
            if name == "__class__":
                object.__setattr__(self, "__class__", After)
                if mode == "returns_old_type_after_bases_mutation":
                    Before.__bases__ = (Expected,)
                    return Before
                if mode == "returns_new_type":
                    return After
                if mode == "returns_old_type":
                    return Before
                if mode == "returns_rejected_finalizer":
                    return RejectedClaim(self)
                return int
            return object.__getattribute__(self, name)

    receiver = Before()
    try:
        result = super(Expected, receiver)
        outcome = ("resolved", result.__self_class__.__name__)
    except BaseException as error:
        outcome = (type(error).__name__, str(error))
    return (
        outcome,
        type(receiver).__name__,
        issubclass(Before, Expected),
        issubclass(type(receiver), Expected),
    )


for mode in (
    "returns_new_type",
    "returns_old_type_after_bases_mutation",
    "returns_old_type",
    "returns_unrelated_type",
    "returns_rejected_finalizer",
):
    print(mode, run_case(mode))
