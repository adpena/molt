# edgebox/events.py -- EventBus with wildcard matching
#
# Provides a publish/subscribe event bus inspired by Django signals.
# Listeners register patterns with optional wildcards:
#   "github.push"   -- exact match
#   "github.*"      -- matches any event starting with "github."
#   "*.push"        -- matches any event ending with ".push"
#   "*"             -- matches everything
#
# No eval, no exec, no metaclasses. Pattern matching uses simple
# string operations only.

from edgebox.types import Event, EventListener


# ---------------------------------------------------------------------------
# Pattern matching
# ---------------------------------------------------------------------------


def _match_pattern(pattern, event_name):
    """Check whether an event name matches a pattern.

    Supports:
        "exact.name"  -- exact equality
        "prefix.*"    -- matches if event starts with "prefix."
        "*.suffix"    -- matches if event ends with ".suffix"
        "*"           -- matches everything
        "a.*.c"       -- matches if first segment is "a" and last is "c"

    Returns True if matched, False otherwise.
    """
    if pattern == "*":
        return True

    if pattern == event_name:
        return True

    # Split both into segments
    pat_parts = pattern.split(".")
    evt_parts = event_name.split(".")

    # Single wildcard segment patterns
    if len(pat_parts) == 1:
        return False  # no dot, not "*", didn't match exact -- fail

    # Walk segments, matching wildcards
    pat_idx = 0
    evt_idx = 0
    while pat_idx < len(pat_parts) and evt_idx < len(evt_parts):
        if pat_parts[pat_idx] == "*":
            # Wildcard matches exactly one segment
            pat_idx = pat_idx + 1
            evt_idx = evt_idx + 1
            continue
        if pat_parts[pat_idx] != evt_parts[evt_idx]:
            return False
        pat_idx = pat_idx + 1
        evt_idx = evt_idx + 1

    # Both must be fully consumed
    return pat_idx == len(pat_parts) and evt_idx == len(evt_parts)


# ---------------------------------------------------------------------------
# EventBus
# ---------------------------------------------------------------------------


class EventBus:
    """Central event bus for publish/subscribe within a box.

    Usage:
        bus = EventBus()

        # Register a listener
        bus.on("github.push", handler_fn)

        # Or use the decorator
        @bus.listener("*.push")
        def on_any_push(box, event):
            ...

        # Emit an event
        bus.emit(box, Event("github.push", {"ref": "main"}))
    """

    def __init__(self):
        self._listeners = []  # list of EventListener

    def on(self, pattern, handler, plugin_name="", priority=100):
        """Register a listener for the given event pattern."""
        listener = EventListener(
            pattern=pattern,
            handler=handler,
            plugin_name=plugin_name,
            priority=priority,
        )
        self._listeners.append(listener)
        self._sort_listeners()
        return listener

    def off(self, pattern=None, handler=None, plugin_name=None):
        """Remove listeners matching the given criteria.

        At least one filter must be provided. All provided filters
        must match for a listener to be removed.
        """
        keep = []
        idx = 0
        while idx < len(self._listeners):
            listener = self._listeners[idx]
            idx = idx + 1
            remove = True
            if pattern is not None and listener.pattern != pattern:
                remove = False
            if handler is not None and listener.handler is not handler:
                remove = False
            if plugin_name is not None and listener.plugin_name != plugin_name:
                remove = False
            if not remove:
                keep.append(listener)
        self._listeners = keep

    def listener(self, pattern, plugin_name="", priority=100):
        """Decorator to register a function as an event listener.

        Usage:
            @bus.listener("github.*")
            def on_github(box, event):
                ...
        """

        def decorator(fn):
            self.on(pattern, fn, plugin_name=plugin_name, priority=priority)
            return fn

        return decorator

    def emit(self, box, event):
        """Emit an event, calling all matching listeners in priority order.

        Args:
            box:   The Box instance (passed as first arg to listeners).
            event: An Event instance, or a string name (auto-wrapped).

        Returns:
            The Event instance (with .is_stopped reflecting cancellation).
        """
        if isinstance(event, str):
            event = Event(event)

        idx = 0
        while idx < len(self._listeners):
            if event.is_stopped:
                break
            listener = self._listeners[idx]
            idx = idx + 1
            if _match_pattern(listener.pattern, event.name):
                listener.handler(box, event)

        return event

    def list_listeners(self, pattern=None):
        """Return a list of registered listeners, optionally filtered."""
        if pattern is None:
            return list(self._listeners)
        result = []
        idx = 0
        while idx < len(self._listeners):
            listener = self._listeners[idx]
            idx = idx + 1
            if listener.pattern == pattern:
                result.append(listener)
        return result

    def _sort_listeners(self):
        """Sort listeners by priority (lower first).

        Uses insertion sort to avoid relying on list.sort() key= parameter
        which may have edge cases in Molt.
        """
        i = 1
        while i < len(self._listeners):
            current = self._listeners[i]
            j = i - 1
            while j >= 0 and self._listeners[j].priority > current.priority:
                self._listeners[j + 1] = self._listeners[j]
                j = j - 1
            self._listeners[j + 1] = current
            i = i + 1
