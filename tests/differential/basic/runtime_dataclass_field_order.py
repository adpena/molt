from dataclasses import dataclass


# Construct the class at runtime so the initializer executes its code-owned plan.
RuntimeStores = dataclass(type("RuntimeStores", (), {
    "__module__": __name__,
    "__annotations__": {"first": object, "second": object},
}))
store_events = []


class RejectSecond:
    def __set__(self, obj, value):
        store_events.append(("second", obj.first, value))
        raise ValueError("second rejected")


class PublishDescriptor:
    def __del__(self):
        store_events.append(("retired",))
        RuntimeStores.second = RejectSecond()


warm_store = RuntimeStores(0, 0)
partial_store = RuntimeStores(PublishDescriptor(), 2)
try:
    RuntimeStores.__init__(partial_store, 3, 4)
except ValueError as exc:
    print("runtime field transaction", str(exc), store_events,
          list(vars(partial_store).items()))
