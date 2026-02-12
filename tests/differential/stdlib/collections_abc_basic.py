"""Purpose: differential coverage for collections.abc basics."""

import collections.abc as abc


class IterOnly:
    def __iter__(self):
        return iter(())


class SizedOnly:
    def __len__(self):
        return 0


class ContainerOnly:
    def __contains__(self, _item):
        return False


class CallableOnly:
    def __call__(self, value=None):
        return value


print(issubclass(IterOnly, abc.Iterable), isinstance(IterOnly(), abc.Iterable))
print(issubclass(SizedOnly, abc.Sized), issubclass(ContainerOnly, abc.Container))
print(
    issubclass(CallableOnly, abc.Callable),
    isinstance(CallableOnly(), abc.Callable),
)
print(issubclass(dict, abc.MutableMapping), isinstance({}, abc.MutableMapping))
print(issubclass(list, abc.MutableSequence), isinstance([], abc.MutableSequence))
print(issubclass(tuple, abc.Sequence), isinstance((), abc.Sequence))
print(
    isinstance({}.keys(), abc.KeysView),
    isinstance({}.items(), abc.ItemsView),
    isinstance({}.values(), abc.ValuesView),
)
