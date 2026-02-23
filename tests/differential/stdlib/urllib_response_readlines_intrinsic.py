"""Purpose: urllib.response handle-backed readlines lowers through runtime intrinsic."""

import urllib.request


with urllib.request.urlopen("data:text/plain,line1%0Aline2%0Aline3") as resp:
    print("all", [line.decode("ascii") for line in resp.readlines()])

with urllib.request.urlopen("data:text/plain,line1%0Aline2%0Aline3") as resp:
    print("hint", [line.decode("ascii") for line in resp.readlines(6)])
    print("tail", resp.read().decode("ascii"))

with urllib.request.urlopen("data:text/plain,line1%0Aline2%0Aline3") as resp:
    print("hint0", [line.decode("ascii") for line in resp.readlines(0)])
