"""Pin wantobjects=1 result-type parity: molt's Tcl_Obj-direct bridge must map
Tcl result types to the same Python types CPython's _tkinter.c FromObj produces.

Runs identically under CPython and molt; prints one line per check. The two
runtimes' outputs must be byte-identical.
"""

import tkinter


def show(label, value):
    print(f"{label} type={type(value).__name__!r} value={value!r}")


def main():
    root = tkinter.Tk()
    root.withdraw()
    tk = root.tk

    # int result: `expr 40+2` -> Python int (not '42').
    show("int", tk.call("expr", "40+2"))
    # large int still int.
    show("int_big", tk.call("expr", "1000000 * 1000000"))
    # negative int.
    show("int_neg", tk.call("expr", "0 - 7"))
    # double result -> Python float.
    show("double", tk.call("expr", "3.5 + 1.25"))
    # boolean-ish: Tcl `expr` of a comparison yields an integer 0/1 (typePtr int),
    # so CPython returns int 1 here (NOT bool). Pin that exact behavior.
    show("expr_cmp", tk.call("expr", "2 > 1"))
    # explicit boolean via `string is`: returns 0/1 int too.
    # list result -> Python tuple of typed elements.
    show("list_str", tk.call("list", "a", "b", "c"))
    show("list_int", tk.call("list", 1, 2, 3))
    # lrange over a built list.
    tk.call("set", "L", tk.call("list", 10, 20, 30, 40))
    show("lrange", tk.call("lrange", tk.call("set", "L"), 1, 2))
    # plain string result.
    tk.call("set", "s", "hello world")
    show("string", tk.call("set", "s"))
    # empty string result.
    show("empty", tk.call("set", "s", ""))
    # string that looks numeric but is set as a string stays a string on read of a
    # freshly-set string var (Tcl keeps the int rep if assigned an int literal via
    # expr, but `set` of a bareword keeps string). Pin observed behavior.
    tk.call("set", "n", "00042")
    show("leading_zero", tk.call("set", "n"))

    # Argument typing round-trip: pass a Python int, read it back.
    tk.call("set", "rt", 12345)
    show("argint_roundtrip", tk.call("set", "rt"))
    # Pass a Python float as an argument; `expr` doubles it -> float result.
    show("argfloat_expr", tk.call("expr", 2.5, "*", 2))
    # Pass a Python bool argument -> Tcl boolean -> expr treats as 1/0.
    show("argbool_expr", tk.call("expr", True, "+", 0))

    root.destroy()
    print("DONE")


if __name__ == "__main__":
    main()
