"""Purpose: differential coverage for typing newtype."""

from typing import NewType


UserId = NewType("UserId", int)
uid = UserId(5)
print(uid, type(uid) is int)
