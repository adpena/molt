"""Purpose: nested indexed loops should execute correctly under native codegen."""


matrix = [[1, 2], [3, 4], [5, 6]]
total = 0

for i in range(len(matrix)):
    row = matrix[i]
    for j in range(len(row)):
        total = total + row[j]

print(total)
