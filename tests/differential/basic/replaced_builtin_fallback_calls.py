"""Replacing each builtin among the runtime-callable lowerings reaches every
read of it (HF-776).

The frontend lowers a proved read of these builtins by name: `ascii`, `bin`,
`hex`, `oct`, `compile`, `divmod`, `format`, `hash`, `dir`, `__import__`.
For each one in turn, a module binding, a `builtins` member write and a
`__builtins__` mapping select another object, so every read below must reach
the replacement, as in CPython. The mapping form comes last: a function takes
its builtins from the mapping when it is created.
"""

import builtins

KINDS = ("code", "module")
NAMES = (
    "ascii",
    "bin",
    "hex",
    "oct",
    "compile",
    "divmod",
    "format",
    "hash",
    "dir",
    "__import__",
)
ORIGINALS = {name: getattr(builtins, name) for name in NAMES}


class Fake:
    """A replacement whose value and call results print deterministically."""

    def __init__(self, name):
        self.name = name

    def __call__(self, *args, **kwargs):
        return "called-" + self.name

    def __repr__(self):
        return "fake-" + self.name


def show(value):
    # Reads no replaceable builtin: the program replaces them.
    return value.__class__.__name__ if value.__class__.__name__ in KINDS else value


class Async:
    def __aiter__(self):
        return self

    def __anext__(self):
        return "anext-result"


def use_ascii():
    return ascii('\xe9')


print("ascii original", show(use_ascii()), show(ascii('\xe9')))
ascii = Fake("ascii-module")
print("ascii module binding", show(use_ascii()), show(ascii('\xe9')))
del ascii
print("ascii module binding deleted", show(use_ascii()), show(ascii('\xe9')))
builtins.ascii = Fake("ascii-member")
print("ascii member", show(use_ascii()), show(ascii('\xe9')))
builtins.ascii = ORIGINALS["ascii"]
print("ascii member restored", show(use_ascii()), show(ascii('\xe9')))


def use_bin():
    return bin(5)


print("bin original", show(use_bin()), show(bin(5)))
bin = Fake("bin-module")
print("bin module binding", show(use_bin()), show(bin(5)))
del bin
print("bin module binding deleted", show(use_bin()), show(bin(5)))
builtins.bin = Fake("bin-member")
print("bin member", show(use_bin()), show(bin(5)))
builtins.bin = ORIGINALS["bin"]
print("bin member restored", show(use_bin()), show(bin(5)))


def use_hex():
    return hex(255)


print("hex original", show(use_hex()), show(hex(255)))
hex = Fake("hex-module")
print("hex module binding", show(use_hex()), show(hex(255)))
del hex
print("hex module binding deleted", show(use_hex()), show(hex(255)))
builtins.hex = Fake("hex-member")
print("hex member", show(use_hex()), show(hex(255)))
builtins.hex = ORIGINALS["hex"]
print("hex member restored", show(use_hex()), show(hex(255)))


def use_oct():
    return oct(8)


print("oct original", show(use_oct()), show(oct(8)))
oct = Fake("oct-module")
print("oct module binding", show(use_oct()), show(oct(8)))
del oct
print("oct module binding deleted", show(use_oct()), show(oct(8)))
builtins.oct = Fake("oct-member")
print("oct member", show(use_oct()), show(oct(8)))
builtins.oct = ORIGINALS["oct"]
print("oct member restored", show(use_oct()), show(oct(8)))


def use_compile():
    return compile('1', '<s>', 'eval')


print("compile original", show(use_compile()), show(compile('1', '<s>', 'eval')))
compile = Fake("compile-module")
print("compile module binding", show(use_compile()), show(compile('1', '<s>', 'eval')))
del compile
print("compile module binding deleted", show(use_compile()), show(compile('1', '<s>', 'eval')))
builtins.compile = Fake("compile-member")
print("compile member", show(use_compile()), show(compile('1', '<s>', 'eval')))
builtins.compile = ORIGINALS["compile"]
print("compile member restored", show(use_compile()), show(compile('1', '<s>', 'eval')))


def use_divmod():
    return divmod(7, 2)


print("divmod original", show(use_divmod()), show(divmod(7, 2)))
divmod = Fake("divmod-module")
print("divmod module binding", show(use_divmod()), show(divmod(7, 2)))
del divmod
print("divmod module binding deleted", show(use_divmod()), show(divmod(7, 2)))
builtins.divmod = Fake("divmod-member")
print("divmod member", show(use_divmod()), show(divmod(7, 2)))
builtins.divmod = ORIGINALS["divmod"]
print("divmod member restored", show(use_divmod()), show(divmod(7, 2)))


def use_format():
    return format(3.25, '.1f')


print("format original", show(use_format()), show(format(3.25, '.1f')))
format = Fake("format-module")
print("format module binding", show(use_format()), show(format(3.25, '.1f')))
del format
print("format module binding deleted", show(use_format()), show(format(3.25, '.1f')))
builtins.format = Fake("format-member")
print("format member", show(use_format()), show(format(3.25, '.1f')))
builtins.format = ORIGINALS["format"]
print("format member restored", show(use_format()), show(format(3.25, '.1f')))


def use_hash():
    return hash(5)


print("hash original", show(use_hash()), show(hash(5)))
hash = Fake("hash-module")
print("hash module binding", show(use_hash()), show(hash(5)))
del hash
print("hash module binding deleted", show(use_hash()), show(hash(5)))
builtins.hash = Fake("hash-member")
print("hash member", show(use_hash()), show(hash(5)))
builtins.hash = ORIGINALS["hash"]
print("hash member restored", show(use_hash()), show(hash(5)))


def use_dir():
    return '__class__' in dir(Async())


print("dir original", show(use_dir()), show('__class__' in dir(Async())))
dir = Fake("dir-module")
print("dir module binding", show(use_dir()), show('__class__' in dir(Async())))
del dir
print("dir module binding deleted", show(use_dir()), show('__class__' in dir(Async())))
builtins.dir = Fake("dir-member")
print("dir member", show(use_dir()), show('__class__' in dir(Async())))
builtins.dir = ORIGINALS["dir"]
print("dir member restored", show(use_dir()), show('__class__' in dir(Async())))


def use___import__():
    return __import__("sys")


print("__import__ original", show(use___import__()), show(__import__("sys")))
__import__ = Fake("__import__-module")
print("__import__ module binding", show(use___import__()), show(__import__("sys")))
del __import__
print(
    "__import__ module binding deleted",
    show(use___import__()),
    show(__import__("sys")),
)
builtins.__import__ = Fake("__import__-member")
print("__import__ member", show(use___import__()), show(__import__("sys")))
builtins.__import__ = ORIGINALS["__import__"]
print("__import__ member restored", show(use___import__()), show(__import__("sys")))

custom = dict(builtins.__dict__)
custom["ascii"] = Fake("ascii-mapping")
custom["bin"] = Fake("bin-mapping")
custom["hex"] = Fake("hex-mapping")
custom["oct"] = Fake("oct-mapping")
custom["compile"] = Fake("compile-mapping")
custom["divmod"] = Fake("divmod-mapping")
custom["format"] = Fake("format-mapping")
custom["hash"] = Fake("hash-mapping")
custom["dir"] = Fake("dir-mapping")
custom["__import__"] = Fake("__import__-mapping")
__builtins__ = custom


def after_ascii():
    return ascii('\xe9')


print("ascii mapping", show(use_ascii()), show(after_ascii()))


def after_bin():
    return bin(5)


print("bin mapping", show(use_bin()), show(after_bin()))


def after_hex():
    return hex(255)


print("hex mapping", show(use_hex()), show(after_hex()))


def after_oct():
    return oct(8)


print("oct mapping", show(use_oct()), show(after_oct()))


def after_compile():
    return compile('1', '<s>', 'eval')


print("compile mapping", show(use_compile()), show(after_compile()))


def after_divmod():
    return divmod(7, 2)


print("divmod mapping", show(use_divmod()), show(after_divmod()))


def after_format():
    return format(3.25, '.1f')


print("format mapping", show(use_format()), show(after_format()))


def after_hash():
    return hash(5)


print("hash mapping", show(use_hash()), show(after_hash()))


def after_dir():
    return '__class__' in dir(Async())


print("dir mapping", show(use_dir()), show(after_dir()))


def after___import__():
    return __import__("sys")


print("__import__ mapping", show(use___import__()), show(after___import__()))
