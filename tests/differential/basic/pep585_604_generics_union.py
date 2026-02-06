"""Purpose: differential coverage for PEP 585 generics + PEP 604 unions."""


alias_list = list[int]
alias_dict = dict[str, int]
union_type = int | str

print(type(alias_list).__name__, repr(alias_list))
print(type(alias_dict).__name__, repr(alias_dict))
print(type(union_type).__name__, repr(union_type))
print(union_type.__args__)
print(repr(list[int | str]))
print(isinstance(alias_list, type(list[int])))
