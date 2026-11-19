"""Purpose: CPython parity for the complete public generational-GC authority."""

import gc


gc.disable()
gc.collect()

referent_tuple = (17, "edge")
referents = gc.get_referents(referent_tuple)
print("referents_inline", 17 in referents, "edge" in referents)

target = []
holder = [target]
print("referrers", any(referrer is holder for referrer in gc.get_referrers(target)))
wide_targets = [object() for _ in range(257)]
wide_holder = [wide_targets[-1]]
print(
    "referrers_wide",
    any(referrer is wide_holder for referrer in gc.get_referrers(*wide_targets)),
)
print("objects", holder in gc.get_objects(), isinstance(gc.get_objects(0), list))

events = []


def callback(phase, info):
    events.append(
        (
            phase,
            [(key, type(value).__name__) for key, value in info.items()],
            info["generation"],
            isinstance(info["collected"], int),
            info["uncollectable"],
            # 3.15: the start phase reports a zeroed collection.
            info.get("candidates", 0) > 0,
            info.get("duration", 0.0) >= 0.0,
        )
    )


gc.callbacks.append(callback)
callback_young = [[] for _ in range(16)]
gc.collect(1)
gc.callbacks.remove(callback)
del callback_young
print("callbacks", len(events))
for event in events:
    print("callback", event)

before_freeze = gc.get_freeze_count()
gc.freeze()
frozen = gc.get_freeze_count()
gc.unfreeze()
print(
    "freeze",
    frozen >= before_freeze,
    frozen > 0,
    gc.get_freeze_count() == 0,
)

resurrected = []


class Finalized:
    def __del__(self):
        resurrected.append(self)


value = Finalized()
value.self = value
del value
gc.collect()
print("finalized", len(resurrected), gc.is_finalized(resurrected[0]))
resurrected.clear()
gc.collect()

previous_debug = gc.get_debug()
start_garbage = len(gc.garbage)
left = []
right = []
left.append(right)
right.append(left)
del left, right
gc.set_debug(gc.DEBUG_SAVEALL)
saved = gc.collect()
print("saveall", saved, len(gc.garbage) - start_garbage)
del gc.garbage[start_garbage:]
gc.set_debug(previous_debug)
gc.collect()

print(
    "constants",
    gc.DEBUG_STATS,
    gc.DEBUG_COLLECTABLE,
    gc.DEBUG_UNCOLLECTABLE,
    gc.DEBUG_SAVEALL,
    gc.DEBUG_LEAK,
)
gc.enable()
