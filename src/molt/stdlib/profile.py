"""Minimal `profile` subset for Molt."""

from __future__ import annotations


class Profile:
    def runctx(self, code: str, globals_dict: dict, locals_dict: dict) -> None:
        exec(code, globals_dict, locals_dict)


__all__ = ["Profile"]
