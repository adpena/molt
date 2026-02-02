"""Purpose: differential coverage for typing typeddict."""

from typing import TypedDict, get_type_hints


class Movie(TypedDict):
    title: str
    year: int


print(get_type_hints(Movie))
