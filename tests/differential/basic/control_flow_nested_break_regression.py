"""Purpose: regression for nested indexed-loop break/else backend lowering."""

broke = False

for i in range(3):
    for j in range(3):
        if i == 1 and j == 1:
            broke = True
            break
    else:
        continue

print("broke", broke)
