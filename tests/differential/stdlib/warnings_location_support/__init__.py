"""A second module whose warnings carry their own location and registry."""

import warnings


def warn_here():
    warnings.warn("support level one", UserWarning)


def warn_caller():
    warnings.warn("support level two", UserWarning, stacklevel=2)


def deprecated_here():
    warnings.warn("support deprecated", DeprecationWarning)


def deprecated_caller():
    warnings.warn("caller deprecated", DeprecationWarning, stacklevel=2)
