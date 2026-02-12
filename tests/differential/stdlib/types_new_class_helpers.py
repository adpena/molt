"""Purpose: differential coverage for types.new_class helper functions."""

import types

events = []


class Adapter:
    def __mro_entries__(self, bases):
        events.append(("mro_entries", len(bases)))
        return (dict,)


class Meta(type):
    @classmethod
    def __prepare__(mcls, name, bases, **kwds):
        events.append(
            (
                "prepare",
                name,
                tuple(
                    base.__name__ if isinstance(base, type) else type(base).__name__
                    for base in bases
                ),
                tuple(sorted(kwds.items())),
            )
        )
        return {}


def exec_body(ns):
    ns["payload"] = 42


base_input = (Adapter(),)
resolved = types.resolve_bases(base_input)
print("resolved_tuple", isinstance(resolved, tuple), resolved[0].__name__)

unchanged = [int]
print("resolve_unchanged_identity", types.resolve_bases(unchanged) is unchanged)

meta, namespace, kwds = types.prepare_class(
    "Prep", (dict,), {"metaclass": Meta, "flag": 7}
)
print(
    "prepare_result",
    meta is Meta,
    isinstance(namespace, dict),
    tuple(sorted(kwds.items())),
)

dyn = types.new_class("Dyn", base_input, {"metaclass": Meta}, exec_body)
print("new_class_bases", tuple(base.__name__ for base in dyn.__bases__))
print("new_class_orig", types.get_original_bases(dyn) == base_input)
print("new_class_payload", dyn.payload)
print("events", events)

try:
    types.get_original_bases(1)
except Exception as exc:  # pragma: no cover - exercised in differential harness
    print("get_original_bases_error", type(exc).__name__)


class BadAdapter:
    def __mro_entries__(self, bases):
        return [dict]


try:
    types.resolve_bases((BadAdapter(),))
except Exception as exc:  # pragma: no cover - exercised in differential harness
    print("resolve_error", type(exc).__name__)
