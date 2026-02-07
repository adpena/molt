"""Purpose: differential coverage for os.pipe fd shape + inheritable defaults."""

import os

rfd, wfd = os.pipe()
try:
    print("pipe_ints", isinstance(rfd, int), isinstance(wfd, int))
    print("pipe_distinct", rfd != wfd)
    print("pipe_inheritable", os.get_inheritable(rfd), os.get_inheritable(wfd))
finally:
    os.close(rfd)
    os.close(wfd)
