"""Intrinsic-backed `tkinter.filedialog` wrappers."""

import fnmatch
import os

import tkinter as _tkinter
from _intrinsics import require_intrinsic as _require_intrinsic
from tkinter import commondialog as _commondialog
from tkinter import dialog as _dialog

_MOLT_TK_FILEDIALOG_SHOW = _require_intrinsic("molt_tk_filedialog_show", globals())

Dialog = _dialog.Dialog
commondialog = _commondialog
dialogstates = {}

Tk = getattr(_tkinter, "Tk", object)
Toplevel = getattr(_tkinter, "Toplevel", object)
Frame = getattr(_tkinter, "Frame", object)
Button = getattr(_tkinter, "Button", object)
Entry = getattr(_tkinter, "Entry", object)
Listbox = getattr(_tkinter, "Listbox", object)
Scrollbar = getattr(_tkinter, "Scrollbar", object)

BOTH = getattr(_tkinter, "BOTH", "both")
BOTTOM = getattr(_tkinter, "BOTTOM", "bottom")
END = getattr(_tkinter, "END", "end")
LEFT = getattr(_tkinter, "LEFT", "left")
RIGHT = getattr(_tkinter, "RIGHT", "right")
TOP = getattr(_tkinter, "TOP", "top")
X = getattr(_tkinter, "X", "x")
Y = getattr(_tkinter, "Y", "y")
YES = getattr(_tkinter, "YES", 1)


def _split_path(path):
    text = str(path)
    slash = text.rfind("/")
    backslash = text.rfind("\\")
    index = slash if slash > backslash else backslash
    if index < 0:
        return ("", text)
    head = text[:index]
    tail = text[index + 1 :]
    if not head and index == 0:
        head = text[:1]
    return (head, tail)


class _Dialog(_commondialog.Dialog):
    command = ""

    def show(self, **options):
        if options:
            self.options.update(options)
        if not self.command:
            raise RuntimeError("dialog command is not configured")
        master = _commondialog._resolve_master(
            self.master,
            role="filedialog master",
        )
        self._fixoptions()
        result = _MOLT_TK_FILEDIALOG_SHOW(
            _commondialog._app_handle(master),
            str(master),
            self.command,
            _commondialog._prepare_intrinsic_options(self.options),
        )
        return self._fixresult(master, result)

    def _fixoptions(self):
        filetypes = self.options.get("filetypes")
        if filetypes is not None:
            self.options["filetypes"] = tuple(filetypes)

    def _fixresult(self, widget, result):
        del widget
        if result:
            value = getattr(result, "string", result)
            value = str(value)
            directory, filename = _split_path(value)
            if directory:
                self.options["initialdir"] = directory
            if filename:
                self.options["initialfile"] = filename
            result = value
        self.filename = result
        return result


class Open(_Dialog):
    command = "tk_getOpenFile"

    def _fixresult(self, widget, result):
        if self.options.get("multiple"):
            if not result:
                return ()
            if isinstance(result, (tuple, list)):
                values = tuple(getattr(item, "string", item) for item in result)
            else:
                values = tuple(widget.splitlist(result))
            if values:
                directory, _ = _split_path(values[0])
                if directory:
                    self.options["initialdir"] = directory
            return tuple(str(item) for item in values)
        return super()._fixresult(widget, result)


class SaveAs(_Dialog):
    command = "tk_getSaveFile"


class Directory(_Dialog):
    command = "tk_chooseDirectory"

    def _fixresult(self, widget, result):
        del widget
        if result:
            value = getattr(result, "string", result)
            value = str(value)
            self.options["initialdir"] = value
            self.directory = value
            return value
        self.directory = result
        return result


class FileDialog:
    """Compatibility shim that routes through native open-file dialogs."""

    title = "File Selection Dialog"

    def __init__(self, master, title=None):
        self.master = master
        self._title = self.title if title is None else str(title)
        self.directory = None
        self.pattern = "*"
        self.selection = ""
        self.filename = None
        self.how = None

    def _build_options(self):
        options = {}
        if self.master is not None:
            options["parent"] = self.master
        if self._title:
            options["title"] = self._title
        if self.directory:
            options["initialdir"] = self.directory
        if self.selection:
            options["initialfile"] = self.selection
        if self.pattern and self.pattern != "*":
            options["filetypes"] = (("Files", self.pattern), ("All Files", "*"))
        return options

    def _show_filename(self):
        return askopenfilename(**self._build_options())

    def go(self, dir_or_file=".", pattern="*", default="", key=None):
        if key and key in dialogstates:
            self.directory, self.pattern = dialogstates[key]
        else:
            if dir_or_file not in (None, ""):
                self.directory = str(dir_or_file)
            self.pattern = "*" if pattern in (None, "") else str(pattern)
        self.selection = "" if default is None else str(default)
        result = self._show_filename()
        self.filename = result
        self.how = result if result else None
        if key is not None:
            dialogstates[key] = (self.directory, self.pattern)
        return self.how

    def quit(self, how=None):
        self.how = how
        self.filename = how
        return None

    def dirs_double_event(self, event=None):
        del event
        self.filter_command()
        return None

    def dirs_select_event(self, event=None):
        del event
        return None

    def files_double_event(self, event=None):
        del event
        self.ok_command()
        return None

    def files_select_event(self, event=None):
        del event
        return None

    def ok_event(self, event=None):
        del event
        self.ok_command()
        return None

    def ok_command(self):
        self.quit(self.get_selection())
        return None

    def filter_command(self, event=None):
        del event
        return None

    def get_filter(self):
        return (self.directory or "", self.pattern or "*")

    def get_selection(self):
        return self.selection

    def cancel_command(self, event=None):
        del event
        self.quit()
        return None

    def set_filter(self, directory, pattern):
        self.directory = "" if directory is None else str(directory)
        self.pattern = "*" if pattern in (None, "") else str(pattern)
        return None

    def set_selection(self, file):
        self.selection = "" if file is None else str(file)
        return None


class LoadFileDialog(FileDialog):
    title = "Load File Selection Dialog"

    def go(self, dir_or_file=".", pattern="*", default="", key=None):
        result = super().go(
            dir_or_file=dir_or_file,
            pattern=pattern,
            default=default,
            key=key,
        )
        if not result:
            return None
        return result

    def ok_command(self):
        file = self.get_selection()
        if not file or not os.path.isfile(file):
            bell = getattr(self.master, "bell", None)
            if callable(bell):
                bell()
            return None
        self.quit(file)
        return None


class SaveFileDialog(FileDialog):
    title = "Save File Selection Dialog"

    def _show_filename(self):
        return asksaveasfilename(**self._build_options())

    def ok_command(self):
        file = self.get_selection()
        if os.path.exists(file):
            if os.path.isdir(file):
                bell = getattr(self.master, "bell", None)
                if callable(bell):
                    bell()
                return None
            dialog = _dialog.Dialog(
                master=self.master,
                title="Overwrite Existing File Question",
                text=f"Overwrite existing file {file!r}?",
                bitmap="questhead",
                default=1,
                strings=("Yes", "Cancel"),
            )
            if dialog.show() != 0:
                return None
        else:
            head, _tail = os.path.split(file)
            if head and not os.path.isdir(head):
                bell = getattr(self.master, "bell", None)
                if callable(bell):
                    bell()
                return None
        self.quit(file)
        return None


def askopenfilename(**options):
    return Open(**options).show()


def asksaveasfilename(**options):
    return SaveAs(**options).show()


def askopenfilenames(**options):
    options["multiple"] = True
    return Open(**options).show()


def askdirectory(**options):
    return Directory(**options).show()


def askopenfile(mode="r", **options):
    filename = askopenfilename(**options)
    if not filename:
        return None
    return open(filename, mode)


def askopenfiles(mode="r", **options):
    files = []
    for filename in askopenfilenames(**options):
        files.append(open(filename, mode))
    return files


def asksaveasfile(mode="w", **options):
    filename = asksaveasfilename(**options)
    if not filename:
        return None
    return open(filename, mode)


def test():
    """Simple compatibility smoke helper."""

    root = Tk()
    if hasattr(root, "withdraw"):
        root.withdraw()

    fd = LoadFileDialog(root)
    loadfile = fd.go(key="test")
    fd = SaveFileDialog(root)
    savefile = fd.go(key="test")
    print(loadfile, savefile)

    openfilename = askopenfilename(filetypes=[("all files", "*")])
    print("open", openfilename)
    saveasfilename = asksaveasfilename()
    print("saveas", saveasfilename)


__all__ = [
    "BOTH",
    "BOTTOM",
    "Button",
    "Dialog",
    "END",
    "Entry",
    "FileDialog",
    "Frame",
    "LEFT",
    "Listbox",
    "LoadFileDialog",
    "RIGHT",
    "SaveFileDialog",
    "Scrollbar",
    "TOP",
    "Tk",
    "Toplevel",
    "X",
    "Y",
    "YES",
    "Directory",
    "Open",
    "SaveAs",
    "askdirectory",
    "askopenfile",
    "askopenfilename",
    "askopenfiles",
    "askopenfilenames",
    "asksaveasfile",
    "asksaveasfilename",
    "commondialog",
    "dialogstates",
    "fnmatch",
    "os",
    "test",
]
