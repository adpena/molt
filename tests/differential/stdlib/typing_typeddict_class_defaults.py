"""Purpose: verify TypedDict class defaults preserve CPython key semantics."""

from typing import TypedDict


class Movie(TypedDict):
    name: str
    year: int = 0


class PartialMovie(TypedDict, total=False):
    name: str
    year: int = 0


print(sorted(Movie.__required_keys__))
print(sorted(Movie.__optional_keys__))
print(Movie.__total__)
print(Movie.__dict__.get("year"))

print(sorted(PartialMovie.__required_keys__))
print(sorted(PartialMovie.__optional_keys__))
print(PartialMovie.__total__)
print(PartialMovie.__dict__.get("year"))
