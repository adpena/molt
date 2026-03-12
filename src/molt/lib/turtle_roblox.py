"""Molt intrinsic module: turtle commands for Roblox Studio.

Usage in molt-compiled Python::

    from molt.lib.turtle_roblox import Turtle
    t = Turtle()
    t.forward(100)       # Move forward 100 studs
    t.right(90)          # Turn right 90 degrees
    t.left(45)           # Turn left 45 degrees
    t.backward(50)       # Move backward 50 studs
    t.sense()            # Sense nearby entities
    t.pen_down()         # Start recording path
    t.pen_up()           # Stop recording path
    t.goto(100, 0, 50)   # Move to absolute position
    t.speed(20)          # Set movement speed (studs/sec)

This is a pure Python stub — the actual Luau lowering happens in the
compiler backend.  The class methods record commands that the IR
pipeline can inspect during compilation.
"""

from __future__ import annotations

__all__ = ["Turtle"]


class Turtle:
    """Roblox-targeted turtle graphics for agent movement."""

    def __init__(self, speed: float = 16.0) -> None:
        self._speed: float = speed
        self._pen: bool = False
        self._commands: list[dict] = []

    # -- movement ----------------------------------------------------------

    def forward(self, distance: float) -> None:
        """Move forward by *distance* studs."""
        self._commands.append(
            {"op": "forward", "distance": distance, "speed": self._speed}
        )

    def backward(self, distance: float) -> None:
        """Move backward by *distance* studs."""
        self._commands.append(
            {"op": "backward", "distance": distance, "speed": self._speed}
        )

    def right(self, angle: float) -> None:
        """Turn right by *angle* degrees."""
        self._commands.append({"op": "right", "angle": angle})

    def left(self, angle: float) -> None:
        """Turn left by *angle* degrees."""
        self._commands.append({"op": "left", "angle": angle})

    def goto(self, x: float, y: float, z: float) -> None:
        """Move to absolute position (*x*, *y*, *z*)."""
        self._commands.append(
            {"op": "goto", "x": x, "y": y, "z": z, "speed": self._speed}
        )

    # -- sensing -----------------------------------------------------------

    def sense(self, radius: float = 50.0) -> list:
        """Sense nearby entities within *radius* studs."""
        self._commands.append({"op": "sense", "radius": radius})
        return []  # Placeholder — runtime fills this

    # -- configuration -----------------------------------------------------

    def speed(self, studs_per_sec: float) -> None:
        """Set movement speed in studs per second."""
        self._speed = studs_per_sec

    # -- pen ---------------------------------------------------------------

    def pen_down(self) -> None:
        """Start recording path."""
        self._pen = True
        self._commands.append({"op": "pen_down"})

    def pen_up(self) -> None:
        """Stop recording path."""
        self._pen = False
        self._commands.append({"op": "pen_up"})

    # -- timing ------------------------------------------------------------

    def wait(self, seconds: float) -> None:
        """Pause execution for *seconds*."""
        self._commands.append({"op": "wait", "seconds": seconds})

    # -- introspection -----------------------------------------------------

    @property
    def commands(self) -> list[dict]:
        """Get a copy of the recorded command sequence."""
        return self._commands.copy()
