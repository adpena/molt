"""Purpose: typing aliases repr, compare and hash as CPython's do.

Python 3.12 and 3.13 spell ``Optional[X]`` as ``typing.Optional[X]``; 3.14
spells every union with ``|``. Class arguments carry their module, ``None``
inside a ``types.UnionType`` repr reads ``None``, and equal aliases hash
equal, whatever their argument order.
"""

import typing
from typing import Annotated, Callable, Dict, List, Literal, Optional, Tuple, Union


class Node:
    pass


print(Optional[int])
print(repr(Optional[int]))
print(Union[int, None])
print(Union[None, int])
print(Union[int, str])
print(Union[int, str, None])
print(Optional[Union[int, str]])
print(Optional[List[int]])
print(Dict[str, Optional[int]])
print(Union[int, Optional[str]])
print(Optional[Optional[int]])
print(Union[int], Union[int, int], Optional[None])
print(List[Node], Optional[Node], Union[Node, int])
print(Dict[str, Node], Tuple[int, ...])
print(Callable[[int], Node], Callable[[], int], Callable[..., int])
print(Callable[[int, str], List[int]])
print(Literal[1, "a"], Annotated[int, "meta"])

print(int | None, None | int, int | str | None)
print(list[int] | None, int | Node)
print(tuple[int, ...], list[None], list[type(None)], dict[str, list[int]])

print(List[int] == List[int], List[int] == List[str])
print(hash(List[int]) == hash(List[int]))
print(Union[int, str] == Union[str, int])
print(hash(Union[int, str]) == hash(Union[str, int]))
print(Optional[int] == (int | None), (int | None) == Optional[int])
print(hash(Optional[int]) == hash(int | None))
print((int | None) == (None | int), hash(int | None) == hash(None | int))
print(Union[int, str] != Union[int, bytes])
print({Optional[int]: "found"}.get(Union[int, None], "missing"))
print(Literal[1] == Literal[1], Literal[1] == Literal[True])
print(hash(Literal[1, 2]) == hash(Literal[2, 1]))
print(Annotated[int, "a"] == Annotated[int, "a"], Annotated[int, "a"] == Annotated[int, "b"])
print(Callable[[int], str] == Callable[[int], str])
print(Callable[[int], str] == Callable[[str], str])
print(typing.get_origin(Optional[int]) is Union, typing.get_args(Optional[int]))
print(Callable[[int], str].__args__, typing.get_args(Callable[[int], str]))
print(typing.get_args(Callable[..., str]), typing.get_args(Callable[[], str]))
