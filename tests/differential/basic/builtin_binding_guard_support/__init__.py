"""Rebinding helpers for builtin_binding_guards.py; a separate module rebinds."""

import builtins


def set_builtin(name, value):
    setattr(builtins, name, value)


def del_builtin(name):
    delattr(builtins, name)


def set_builtin_item(name, value):
    builtins.__dict__[name] = value


def set_module_attr(module, name, value):
    setattr(module, name, value)


def del_module_attr(module, name):
    delattr(module, name)


def set_module_item(module, name, value):
    module.__dict__[name] = value


def call_through(function, *args):
    return function(*args)
