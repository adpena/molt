"""Purpose: an in-place add past the 47-bit inline int range stays an exact int.

Guards a folded constant leaving the inline range (HF-214).
"""

def bump():
    x = 70368744177663      # 2**46 - 1, the inline maximum
    x += 1
    return x


r = bump()
print(r, type(r).__name__, r == 2**46)
