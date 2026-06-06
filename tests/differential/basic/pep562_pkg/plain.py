"""PEP 562 control fixture: a module that defines NEITHER ``__getattr__`` nor
``__dir__``. A missing attribute must raise the stock module ``AttributeError``,
and ``dir()`` must fall back to the default module behaviour (its namespace).
"""

x = 1
y = 2
