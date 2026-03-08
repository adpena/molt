# kdtree_3d.py -- 3D KD-tree for 3D points (Molt --target luau compatible)
# NO imports, NO classes, NO comprehensions, NO lambda, NO module-level mutation.
# NO continue statements (molts Luau backend emits empty block for continue).
# Uses if/else to handle -1 sentinel instead of continue.

def _sq_dist(a, b):
    dx = a[0] - b[0]
    dy = a[1] - b[1]
    dz = a[2] - b[2]
    return dx * dx + dy * dy + dz * dz

def _isort(indices, pts, axis, lo, hi):
    i = lo + 1
    while i <= hi:
        key_idx = indices[i]
        key_val = pts[key_idx][axis]
        j = i - 1
        while j >= lo and pts[indices[j]][axis] > key_val:
            indices[j + 1] = indices[j]
            j = j - 1
        indices[j + 1] = key_idx
        i = i + 1

def _alloc(pool, pt, axis):
    idx = len(pool[0])
    pool[0].append(pt)
    pool[1].append(-1)
    pool[2].append(-1)
    pool[3].append(axis)
    return idx

def _build(pool, pts, indices, lo, hi, depth):
    if lo > hi:
        return -1
    axis = depth % 3
    _isort(indices, pts, axis, lo, hi)
    mid = (lo + hi) // 2
    nid = _alloc(pool, pts[indices[mid]], axis)
    pool[1][nid] = _build(pool, pts, indices, lo, mid - 1, depth + 1)
    pool[2][nid] = _build(pool, pts, indices, mid + 1, hi, depth + 1)
    return nid

def build(points):
    """Return pool = [node_pt, node_left, node_right, node_axis, root]."""
    pool = [[], [], [], [], -1]
    n = len(points)
    if n == 0:
        return pool
    indices = []
    i = 0
    while i < n:
        indices.append(i)
        i = i + 1
    pool[4] = _build(pool, points, indices, 0, n - 1, 0)
    return pool

def nearest(pool, query):
    """Return nearest [x, y, z] to query."""
    root = pool[4]
    if root == -1:
        return []
    npt = pool[0]
    nleft = pool[1]
    nright = pool[2]
    naxis = pool[3]

    best_d2 = _sq_dist(query, npt[root]) + 1.0
    best_pt = npt[root]

    stk = [root]
    sz = 1

    while sz > 0:
        sz = sz - 1
        nid = stk[sz]

        if nid != -1:
            pt = npt[nid]
            d2 = _sq_dist(query, pt)
            if d2 < best_d2:
                best_d2 = d2
                best_pt = pt

            ax = naxis[nid]
            diff = query[ax] - pt[ax]
            diff2 = diff * diff

            if diff <= 0:
                if diff2 < best_d2:
                    right = nright[nid]
                    if right != -1:
                        if sz < len(stk):
                            stk[sz] = right
                        else:
                            stk.append(right)
                        sz = sz + 1
                left = nleft[nid]
                if left != -1:
                    if sz < len(stk):
                        stk[sz] = left
                    else:
                        stk.append(left)
                    sz = sz + 1
            else:
                if diff2 < best_d2:
                    left = nleft[nid]
                    if left != -1:
                        if sz < len(stk):
                            stk[sz] = left
                        else:
                            stk.append(left)
                        sz = sz + 1
                right = nright[nid]
                if right != -1:
                    if sz < len(stk):
                        stk[sz] = right
                    else:
                        stk.append(right)
                    sz = sz + 1

    return best_pt

def range_query(pool, query, radius):
    """Return list of [x, y, z] within radius of query."""
    root = pool[4]
    if root == -1:
        return []
    npt = pool[0]
    nleft = pool[1]
    nright = pool[2]
    naxis = pool[3]
    r2 = radius * radius

    results = []
    stk = [root]
    sz = 1

    while sz > 0:
        sz = sz - 1
        nid = stk[sz]

        if nid != -1:
            pt = npt[nid]
            d2 = _sq_dist(query, pt)
            if d2 <= r2:
                results.append(pt)

            ax = naxis[nid]
            diff = query[ax] - pt[ax]
            diff2 = diff * diff

            if diff <= 0:
                left = nleft[nid]
                if left != -1:
                    if sz < len(stk):
                        stk[sz] = left
                    else:
                        stk.append(left)
                    sz = sz + 1
                if diff2 <= r2:
                    right = nright[nid]
                    if right != -1:
                        if sz < len(stk):
                            stk[sz] = right
                        else:
                            stk.append(right)
                        sz = sz + 1
            else:
                right = nright[nid]
                if right != -1:
                    if sz < len(stk):
                        stk[sz] = right
                    else:
                        stk.append(right)
                    sz = sz + 1
                if diff2 <= r2:
                    left = nleft[nid]
                    if left != -1:
                        if sz < len(stk):
                            stk[sz] = left
                        else:
                            stk.append(left)
                        sz = sz + 1

    return results

# ─── Demo ─────────────────────────────────────────────────────────────────────

pts = [
    [2.0, 3.0, 1.0],
    [5.0, 4.0, 2.0],
    [9.0, 6.0, 7.0],
    [4.0, 7.0, 9.0],
    [8.0, 1.0, 5.0],
    [7.0, 2.0, 3.0],
]

pool = build(pts)
print("built", len(pts), "points")

nn = nearest(pool, [5.0, 5.0, 5.0])
print("nearest to 5,5,5:", nn[0], nn[1], nn[2])

hits = range_query(pool, [5.0, 5.0, 5.0], 4.0)
print("range hits:", len(hits))

nn2 = nearest(pool, [0.0, 0.0, 0.0])
print("nearest to 0,0,0:", nn2[0], nn2[1], nn2[2])

grid = []
ix = 0
while ix < 5:
    iy = 0
    while iy < 5:
        iz = 0
        while iz < 2:
            grid.append([ix * 1.0, iy * 1.0, iz * 1.0])
            iz = iz + 1
        iy = iy + 1
    ix = ix + 1

pool2 = build(grid)
hits2 = range_query(pool2, [2.0, 2.0, 0.5], 1.5)
print("grid range hits:", len(hits2))
