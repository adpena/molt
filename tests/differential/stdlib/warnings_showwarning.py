"""Purpose: differential coverage for warnings showwarning."""

import warnings


messages = []


def showwarning(message, category, filename, lineno, file=None, line=None):
    messages.append((str(message), category.__name__))


warnings.showwarning = showwarning
warnings.warn("hello", UserWarning)
print(messages)
