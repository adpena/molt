"""Raw builtin classmethod-descriptor visibility and callability frontier.

Kept separate from bound hook dispatch so each semantic class has its own
replayable result. This capsule is not a support or conformance claim.
"""


class Subject:
    pass


for label, arguments in (("explicit", (Subject,)), ("missing", ())):
    try:
        result = object.__dict__["__init_subclass__"](*arguments)
    except Exception as error:
        print(label, type(error).__name__)
    else:
        print(label, "ok", result)
