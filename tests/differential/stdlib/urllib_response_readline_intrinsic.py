"""Purpose: urllib.response handle-backed readline lowers through runtime intrinsic."""

import urllib.request


with urllib.request.urlopen("data:text/plain,line1%0Aline2%0Aline3") as resp:
    print("l1", resp.readline().decode("ascii").rstrip())
    print("l2", resp.readline(5).decode("ascii").rstrip())
    print("l2_rest", resp.readline().decode("ascii").rstrip())
    print("tail", resp.read().decode("ascii"))
