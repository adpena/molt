"""Public API surface shim for ``ctypes.wintypes``."""

from __future__ import annotations


from _intrinsics import require_intrinsic as _require_intrinsic

_require_intrinsic("molt_capabilities_has", globals())


class PyCSimpleType(type):
    pass


class PyCPointerType(type):
    pass


class PyCStructType(type):
    pass


def _make_simple(name: str):
    return PyCSimpleType(name, (), {})


def _make_pointer(name: str):
    return PyCPointerType(name, (), {})


def _make_struct(name: str):
    return PyCStructType(name, (), {})


for _name in (
    "ATOM",
    "BOOL",
    "BOOLEAN",
    "BYTE",
    "CHAR",
    "COLORREF",
    "DOUBLE",
    "DWORD",
    "FLOAT",
    "HACCEL",
    "HANDLE",
    "HBITMAP",
    "HBRUSH",
    "HCOLORSPACE",
    "HDC",
    "HDESK",
    "HDWP",
    "HENHMETAFILE",
    "HFONT",
    "HGDIOBJ",
    "HGLOBAL",
    "HHOOK",
    "HICON",
    "HINSTANCE",
    "HKEY",
    "HKL",
    "HLOCAL",
    "HMENU",
    "HMETAFILE",
    "HMODULE",
    "HMONITOR",
    "HPALETTE",
    "HPEN",
    "HRGN",
    "HRSRC",
    "HSTR",
    "HTASK",
    "HWINSTA",
    "HWND",
    "INT",
    "LANGID",
    "LARGE_INTEGER",
    "LCID",
    "LCTYPE",
    "LGRPID",
    "LONG",
    "LPARAM",
    "LPCOLESTR",
    "LPCSTR",
    "LPCVOID",
    "LPCWSTR",
    "LPOLESTR",
    "LPSTR",
    "LPVOID",
    "LPWSTR",
    "OLESTR",
    "SC_HANDLE",
    "SERVICE_STATUS_HANDLE",
    "SHORT",
    "UINT",
    "ULARGE_INTEGER",
    "ULONG",
    "USHORT",
    "VARIANT_BOOL",
    "WCHAR",
    "WORD",
    "WPARAM",
):
    import sys as _wt_sys
    _wt_mod_dict = getattr(_wt_sys.modules.get(__name__), "__dict__", None) or globals()
    _wt_mod_dict[_name] = _make_simple(_name)

for _name in (
    "LPBOOL",
    "LPBYTE",
    "LPCOLORREF",
    "LPDWORD",
    "LPFILETIME",
    "LPHANDLE",
    "LPHKL",
    "LPINT",
    "LPLONG",
    "LPMSG",
    "LPPOINT",
    "LPRECT",
    "LPRECTL",
    "LPSC_HANDLE",
    "LPSIZE",
    "LPSIZEL",
    "LPUINT",
    "LPWIN32_FIND_DATAA",
    "LPWIN32_FIND_DATAW",
    "LPWORD",
    "PBOOL",
    "PBOOLEAN",
    "PBYTE",
    "PCHAR",
    "PDWORD",
    "PFILETIME",
    "PFLOAT",
    "PHANDLE",
    "PHKEY",
    "PINT",
    "PLARGE_INTEGER",
    "PLCID",
    "PLONG",
    "PMSG",
    "PPOINT",
    "PPOINTL",
    "PRECT",
    "PRECTL",
    "PSHORT",
    "PSIZE",
    "PSIZEL",
    "PSMALL_RECT",
    "PUINT",
    "PULARGE_INTEGER",
    "PULONG",
    "PUSHORT",
    "PWCHAR",
    "PWIN32_FIND_DATAA",
    "PWIN32_FIND_DATAW",
    "PWORD",
):
    import sys as _wt_sys
    _wt_mod_dict = getattr(_wt_sys.modules.get(__name__), "__dict__", None) or globals()
    _wt_mod_dict[_name] = _make_pointer(_name)

for _name in (
    "FILETIME",
    "MSG",
    "POINT",
    "POINTL",
    "RECT",
    "RECTL",
    "SIZE",
    "SIZEL",
    "SMALL_RECT",
    "WIN32_FIND_DATAA",
    "WIN32_FIND_DATAW",
    "tagMSG",
    "tagPOINT",
    "tagRECT",
    "tagSIZE",
):
    import sys as _wt_sys
    _wt_mod_dict = getattr(_wt_sys.modules.get(__name__), "__dict__", None) or globals()
    _wt_mod_dict[_name] = _make_struct(_name)

MAX_PATH = 260


def RGB(red: int, green: int, blue: int) -> int:
    return ((int(blue) & 0xFF) << 16) | ((int(green) & 0xFF) << 8) | (int(red) & 0xFF)


del _name
del _make_simple
del _make_pointer
del _make_struct
del PyCSimpleType
del PyCPointerType
del PyCStructType
