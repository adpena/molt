import types
from typing import TypeVar, TypeVarTuple

T = TypeVar("T")
U = TypeVar("U")
Ts = TypeVarTuple("Ts")

alias_list = list[T]
alias_dict = dict[str, T]
alias_tuple_dup = tuple[T, T]
alias_tuple_var = tuple[Ts]

print(isinstance(alias_list, types.GenericAlias))
print(alias_list.__parameters__)
print(alias_dict.__parameters__)
print(alias_tuple_dup.__parameters__)
print(alias_tuple_var.__parameters__)
