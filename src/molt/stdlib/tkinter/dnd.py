"""Intrinsic-backed `tkinter.dnd` wrappers."""

import tkinter as _tkinter
from _intrinsics import require_intrinsic as _require_intrinsic

from ._support import tk_unavailable_message as _tk_unavailable_message

_MOLT_CAPABILITIES_HAS = _require_intrinsic("molt_capabilities_has", globals())
_MOLT_TK_AVAILABLE = _require_intrinsic("molt_tk_available", globals())


def _has_gui_capability():
    return bool(_MOLT_CAPABILITIES_HAS("gui.window")) or bool(
        _MOLT_CAPABILITIES_HAS("gui")
    )


def _require_gui_capability():
    if not _has_gui_capability():
        raise PermissionError("missing gui.window capability")


def _require_tk_runtime(operation):
    if bool(_MOLT_TK_AVAILABLE()):
        return
    raise RuntimeError(_tk_unavailable_message(operation))


class DndHandler:
    """Drag-and-drop state holder compatible with CPython's shape."""

    root = None

    def __init__(self, source, event):
        self.root = None
        self.source = source
        self.target = None
        self.initial_button = getattr(event, "num", None)
        self.initial_widget = getattr(event, "widget", None)
        self.release_pattern = None
        self.save_cursor = ""

        widget = self.initial_widget
        if widget is None:
            return

        root_getter = getattr(widget, "_root", None)
        root = root_getter() if callable(root_getter) else None
        if root is None:
            return

        try:
            root.__dnd  # noqa: B018
            return
        except AttributeError:
            root.__dnd = self
            self.root = root

        button = self.initial_button
        if button is None:
            return

        self.release_pattern = f"<B{button}-ButtonRelease-{button}>"
        try:
            self.save_cursor = widget["cursor"] or ""
        except Exception:  # noqa: BLE001
            self.save_cursor = ""

        bind = getattr(widget, "bind", None)
        if callable(bind):
            bind(self.release_pattern, self.on_release)
            bind("<Motion>", self.on_motion)
        try:
            widget["cursor"] = "hand2"
        except Exception:  # noqa: BLE001
            pass

    def __del__(self):
        root = self.root
        self.root = None
        if root is not None:
            try:
                del root.__dnd
            except AttributeError:
                pass

    def on_motion(self, event):
        widget = self.initial_widget
        if widget is None:
            return None

        x = getattr(event, "x_root", 0)
        y = getattr(event, "y_root", 0)
        containing = getattr(widget, "winfo_containing", None)
        target_widget = containing(x, y) if callable(containing) else None

        source = self.source
        new_target = None
        while target_widget is not None:
            accept = getattr(target_widget, "dnd_accept", None)
            if callable(accept):
                new_target = accept(source, event)
                if new_target is not None:
                    break
            target_widget = getattr(target_widget, "master", None)

        old_target = self.target
        if old_target is new_target:
            motion = getattr(old_target, "dnd_motion", None)
            if callable(motion):
                motion(source, event)
            return old_target

        if old_target is not None:
            self.target = None
            leave = getattr(old_target, "dnd_leave", None)
            if callable(leave):
                leave(source, event)
        if new_target is not None:
            enter = getattr(new_target, "dnd_enter", None)
            if callable(enter):
                enter(source, event)
            self.target = new_target
        return new_target

    def on_release(self, event):
        return self.finish(event, commit=True)

    def cancel(self, event=None):
        return self.finish(event, commit=False)

    def finish(self, event, commit=False):
        target = self.target
        source = self.source
        widget = self.initial_widget
        root = self.root

        try:
            if root is not None:
                try:
                    del root.__dnd
                except AttributeError:
                    pass
            if widget is not None:
                unbind = getattr(widget, "unbind", None)
                if callable(unbind):
                    if self.release_pattern:
                        unbind(self.release_pattern)
                    unbind("<Motion>")
                try:
                    widget["cursor"] = self.save_cursor
                except Exception:  # noqa: BLE001
                    pass

            self.target = None
            self.source = None
            self.initial_widget = None
            self.root = None

            if target is not None:
                if commit:
                    commit_fn = getattr(target, "dnd_commit", None)
                    if callable(commit_fn):
                        commit_fn(source, event)
                else:
                    leave = getattr(target, "dnd_leave", None)
                    if callable(leave):
                        leave(source, event)
        finally:
            end = getattr(source, "dnd_end", None)
            if callable(end):
                end(target, event)
        return target


def dnd_start(source, event):
    if source is None:
        raise TypeError("source must not be None")
    _require_gui_capability()
    _require_tk_runtime("tkinter.dnd.dnd_start")
    handler = DndHandler(source, event)
    if handler.root is not None:
        return handler
    return None


class Icon:
    """Simple draggable icon helper used by tkinter.dnd demos."""

    def __init__(self, name):
        self.name = name
        self.canvas = None
        self.label = None
        self.id = None
        self.x_off = 0
        self.y_off = 0
        self.x_orig = 0
        self.y_orig = 0

    def attach(self, canvas, x=10, y=10):
        if canvas is self.canvas and self.canvas is not None and self.id is not None:
            self.canvas.coords(self.id, x, y)
            return
        if self.canvas is not None:
            self.detach()
        if canvas is None:
            return
        label = _tkinter.Label(canvas, text=self.name, borderwidth=2, relief="raised")
        item_id = canvas.create_window(x, y, window=label, anchor="nw")
        self.canvas = canvas
        self.label = label
        self.id = item_id
        label.bind("<ButtonPress>", self.press)

    def detach(self):
        canvas = self.canvas
        if canvas is None:
            return
        item_id = self.id
        label = self.label
        self.canvas = None
        self.label = None
        self.id = None
        if item_id is not None:
            canvas.delete(item_id)
        if label is not None:
            label.destroy()

    def press(self, event):
        if dnd_start(self, event):
            self.x_off = getattr(event, "x", 0)
            self.y_off = getattr(event, "y", 0)
            if self.canvas is not None and self.id is not None:
                coords = self.canvas.coords(self.id)
                if len(coords) >= 2:
                    self.x_orig, self.y_orig = coords[:2]

    def move(self, event):
        if self.canvas is None or self.id is None:
            return
        x, y = self.where(self.canvas, event)
        self.canvas.coords(self.id, x, y)

    def putback(self):
        if self.canvas is None or self.id is None:
            return
        self.canvas.coords(self.id, self.x_orig, self.y_orig)

    def where(self, canvas, event):
        x_org = canvas.winfo_rootx()
        y_org = canvas.winfo_rooty()
        x = getattr(event, "x_root", 0) - x_org
        y = getattr(event, "y_root", 0) - y_org
        return x - self.x_off, y - self.y_off

    def dnd_end(self, target, event):
        del target, event
        return None


class Tester:
    """Small target surface used by tkinter.dnd.test()."""

    def __init__(self, root):
        self.top = _tkinter.Toplevel(root)
        self.canvas = _tkinter.Canvas(self.top, width=100, height=100)
        self.canvas.pack(fill="both", expand=1)
        self.canvas.dnd_accept = self.dnd_accept
        self.dndid = None

    def dnd_accept(self, source, event):
        del source, event
        return self

    def dnd_enter(self, source, event):
        self.canvas.focus_set()
        x, y = source.where(self.canvas, event)
        x1, y1, x2, y2 = source.canvas.bbox(source.id)
        dx, dy = x2 - x1, y2 - y1
        self.dndid = self.canvas.create_rectangle(x, y, x + dx, y + dy)
        self.dnd_motion(source, event)

    def dnd_motion(self, source, event):
        if self.dndid is None:
            return
        x, y = source.where(self.canvas, event)
        x1, y1, x2, y2 = self.canvas.bbox(self.dndid)
        self.canvas.move(self.dndid, x - x1, y - y1)

    def dnd_leave(self, source, event):
        del source, event
        self.top.focus_set()
        if self.dndid is not None:
            self.canvas.delete(self.dndid)
            self.dndid = None

    def dnd_commit(self, source, event):
        self.dnd_leave(source, event)
        x, y = source.where(self.canvas, event)
        source.attach(self.canvas, x, y)


def test():
    _require_gui_capability()
    _require_tk_runtime("tkinter.dnd.test")
    root = _tkinter.Tk()
    root.geometry("+1+1")
    _tkinter.Button(command=root.quit, text="Quit").pack()
    t1 = Tester(root)
    t1.top.geometry("+1+60")
    t2 = Tester(root)
    t2.top.geometry("+120+60")
    t3 = Tester(root)
    t3.top.geometry("+240+60")
    i1 = Icon("ICON1")
    i2 = Icon("ICON2")
    i3 = Icon("ICON3")
    i1.attach(t1.canvas)
    i2.attach(t2.canvas)
    i3.attach(t3.canvas)
    root.mainloop()


__all__ = ["DndHandler", "Icon", "Tester", "dnd_start", "test"]
