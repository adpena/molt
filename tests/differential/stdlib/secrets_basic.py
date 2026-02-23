"""Purpose: differential coverage for secrets basic."""

import secrets

# token_bytes returns bytes of the requested length
tb = secrets.token_bytes(16)
print("token_bytes_type", isinstance(tb, bytes))
print("token_bytes_len", len(tb) == 16)

# token_hex returns a hex string of twice the byte length
th = secrets.token_hex(16)
print("token_hex_type", isinstance(th, str))
print("token_hex_len", len(th) == 32)
# Verify all characters are valid hex
print("token_hex_valid", all(c in "0123456789abcdef" for c in th))

# compare_digest returns True for equal strings
print("compare_digest_equal", secrets.compare_digest("abc", "abc"))
print("compare_digest_unequal", not secrets.compare_digest("abc", "xyz"))
print("compare_digest_bytes", secrets.compare_digest(b"hello", b"hello"))

# randbelow returns an int in [0, upper)
rb = secrets.randbelow(100)
print("randbelow_type", isinstance(rb, int))
print("randbelow_range", 0 <= rb < 100)

# choice picks an element from a sequence
items = [10, 20, 30, 40, 50]
c = secrets.choice(items)
print("choice_in_items", c in items)

# token_bytes with default length
tb_default = secrets.token_bytes()
print("token_bytes_default_type", isinstance(tb_default, bytes))
print("token_bytes_default_len", len(tb_default) == 32)
