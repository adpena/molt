import sys

import pytest

import molt_buffer


def snapshot(buffer):
    return [
        [buffer.get(row, col) for col in range(buffer.cols)]
        for row in range(buffer.rows)
    ]


def test_constructor_new_and_dimensions_share_one_wrapper_contract():
    direct = molt_buffer.Buffer2D(2, 3, 4)
    factory = molt_buffer.new(2, 3, 4)

    assert direct.rows == factory.rows == 2
    assert direct.cols == factory.cols == 3
    assert snapshot(direct) == snapshot(factory) == [[4, 4, 4], [4, 4, 4]]

    with pytest.raises(AttributeError):
        direct.rows = 9
    with pytest.raises(AttributeError):
        direct.cols = 9


def test_negative_dimensions_and_indexes_match_list_semantics():
    with pytest.raises(ValueError):
        molt_buffer.new(-1, 2)
    with pytest.raises(ValueError):
        molt_buffer.new(1, -2)
    with pytest.raises(OverflowError):
        molt_buffer.new(0, sys.maxsize + 1)

    wide_empty = molt_buffer.new(0, sys.maxsize)
    assert (wide_empty.rows, wide_empty.cols) == (0, sys.maxsize)
    assert snapshot(wide_empty) == []

    tall_empty = molt_buffer.new(sys.maxsize, 0)
    assert (tall_empty.rows, tall_empty.cols) == (sys.maxsize, 0)
    with pytest.raises(IndexError, match="buffer2d index out of range"):
        tall_empty.get(-1, 0)
    with pytest.raises(IndexError, match="buffer2d index out of range"):
        tall_empty.set(-1, 0, 7)

    buffer = molt_buffer.new(2, 2)
    assert buffer.set(-1, -1, 7) is None
    assert buffer.get(-1, -1) == 7
    with pytest.raises(IndexError):
        buffer.get(-3, 0)
    with pytest.raises(IndexError):
        buffer.get(2, 0)


def test_module_set_returns_same_wrapper_and_rejects_wrong_objects():
    buffer = molt_buffer.new(1, 1)
    assert molt_buffer.set(buffer, 0, 0, 11) is buffer
    assert molt_buffer.get(buffer, 0, 0) == 11

    with pytest.raises(TypeError):
        molt_buffer.get(object(), 0, 0)
    with pytest.raises(TypeError):
        molt_buffer.set(object(), 0, 0, 1)
    with pytest.raises(TypeError):
        molt_buffer.matmul(buffer, object())


def test_integer_index_protocol_is_shared_by_init_and_set():
    class IndexValue:
        def __init__(self, value):
            self.value = value

        def __index__(self):
            return self.value

    buffer = molt_buffer.new(IndexValue(1), IndexValue(2), IndexValue(5))
    assert snapshot(buffer) == [[5, 5]]
    assert buffer.set(0, 1, IndexValue(9)) is None
    assert snapshot(buffer) == [[5, 9]]

    bool_buffer = molt_buffer.new(True, True, True)
    assert (bool_buffer.rows, bool_buffer.cols, bool_buffer.get(0, 0)) == (1, 1, 1)

    with pytest.raises(TypeError):
        molt_buffer.new(1, 1, 1.5)
    with pytest.raises(TypeError):
        buffer.set(0, 0, 1.5)


def test_set_validates_value_before_indexes_in_both_execution_modes():
    buffer = molt_buffer.new(1, 1)
    with pytest.raises(TypeError):
        buffer.set(99, 99, 1.5)


@pytest.mark.parametrize("cols", [1, sys.maxsize])
def test_impossible_cell_count_or_byte_extent_fails_before_backing_allocation(cols):
    with pytest.raises(MemoryError, match="^buffer2d allocation failed$"):
        molt_buffer.new(sys.maxsize, cols)


def test_reinitialization_replaces_shape_and_storage_together():
    buffer = molt_buffer.new(2, 3, 7)
    buffer.__init__(3, 2, 9)
    assert (buffer.rows, buffer.cols) == (3, 2)
    assert snapshot(buffer) == [[9, 9], [9, 9], [9, 9]]
    with pytest.raises(MemoryError):
        buffer.__init__(sys.maxsize, sys.maxsize)
    assert (buffer.rows, buffer.cols) == (3, 2)
    assert snapshot(buffer) == [[9, 9], [9, 9], [9, 9]]


@pytest.mark.parametrize("axis", [0, 1])
@pytest.mark.parametrize("write", [False, True])
def test_index_callback_reinitialization_keeps_evaluated_storage_alive(axis, write):
    buffer = molt_buffer.new(2, 3, 7)

    class ReinitializingIndex:
        def __index__(self):
            buffer.__init__(1, 1, 9)
            return -1

    indices = [-1, -1]
    indices[axis] = ReinitializingIndex()
    if write:
        assert buffer.set(*indices, 11) is None
    else:
        assert buffer.get(*indices) == 7
    assert (buffer.rows, buffer.cols) == (1, 1)
    assert snapshot(buffer) == [[9]]


@pytest.mark.parametrize("index", [slice(None), slice(0, 1), 0.5, "0", None])
@pytest.mark.parametrize("axis", [0, 1])
@pytest.mark.parametrize("write", [False, True])
def test_get_and_set_reject_non_integer_indices(index, axis, write):
    buffer = molt_buffer.new(1, 1, 7)
    indices = [0, 0]
    indices[axis] = index
    with pytest.raises(TypeError, match="cannot be interpreted as an integer"):
        if write:
            buffer.set(*indices, 8)
        else:
            buffer.get(*indices)
    assert buffer.get(0, 0) == 7


@pytest.mark.parametrize("write", [False, True])
def test_index_callbacks_and_bounds_are_checked_in_source_order(write):
    buffer = molt_buffer.new(1, 1, 7)
    events = []

    class Index:
        def __init__(self, name, value):
            self.name = name
            self.value = value

        def __index__(self):
            events.append(self.name)
            return self.value

    def access(row, col):
        if write:
            buffer.set(row, col, Index("value", 9))
        else:
            buffer.get(row, col)

    prefix = ["value"] if write else []
    access(Index("row", 0), Index("col", 0))
    assert events == prefix + ["row", "col"]
    events.clear()
    with pytest.raises(IndexError, match="buffer2d index out of range"):
        access(Index("row", 1), Index("col", 0))
    assert events == prefix + ["row"]
    events.clear()
    with pytest.raises(TypeError, match="cannot be interpreted as an integer"):
        access(slice(None), Index("col", 0))
    assert events == prefix


@pytest.mark.parametrize("write", [False, True])
def test_index_callback_exceptions_are_preserved(write):
    buffer = molt_buffer.new(1, 1, 7)
    failure = TypeError("index callback failed")

    class BrokenIndex:
        def __index__(self):
            raise failure

    with pytest.raises(TypeError) as raised:
        if write:
            buffer.set(BrokenIndex(), 0, 9)
        else:
            buffer.get(0, BrokenIndex())
    assert raised.value is failure
    assert buffer.get(0, 0) == 7


def test_arbitrary_precision_roundtrip_and_matmul_do_not_wrap():
    large = (1 << 90) + 3
    left = molt_buffer.new(1, 2)
    right = molt_buffer.new(2, 1)
    molt_buffer.set(left, 0, 0, large)
    molt_buffer.set(left, 0, 1, -large)
    molt_buffer.set(right, 0, 0, large)
    molt_buffer.set(right, 1, 0, 2)

    assert left.get(0, 0) == large
    assert snapshot(molt_buffer.matmul(left, right)) == [[large * large - 2 * large]]


def test_matmul_mismatch_and_zero_dimensions():
    with pytest.raises(ValueError):
        molt_buffer.matmul(molt_buffer.new(2, 3), molt_buffer.new(2, 1))

    empty = molt_buffer.new(0, 3)
    assert empty.rows == 0
    assert empty.cols == 3
    assert snapshot(empty) == []

    for rows, cols in ((0, sys.maxsize), (sys.maxsize, 0)):
        product = molt_buffer.matmul(molt_buffer.new(rows, 0), molt_buffer.new(0, cols))
        assert (product.rows, product.cols) == (rows, cols)

    product = molt_buffer.matmul(molt_buffer.new(2, 0), molt_buffer.new(0, 3))
    assert snapshot(product) == [[0, 0, 0], [0, 0, 0]]


@pytest.mark.parametrize("virtual_left", [False, True])
@pytest.mark.parametrize("initialize_storage", [False, True])
def test_matmul_uses_virtual_shape_and_cells(virtual_left, initialize_storage):
    class VirtualBuffer(molt_buffer.Buffer2D):
        def __init__(self):
            if initialize_storage:
                super().__init__(3, 4, 99)

        @property
        def rows(self):
            return 1

        @property
        def cols(self):
            return 1

        def get(self, row, col):
            assert (row, col) == (0, 0)
            return 7

    ordinary = molt_buffer.new(1, 1, 3)
    virtual = VirtualBuffer()
    left, right = (virtual, ordinary) if virtual_left else (ordinary, virtual)
    product = molt_buffer.matmul(left, right)
    assert type(product) is molt_buffer.Buffer2D
    assert snapshot(product) == [[21]]


def test_matmul_observes_instance_shadows_and_reentrant_get_replacement():
    left = molt_buffer.new(1, 2, 99)
    right = molt_buffer.new(2, 1, 5)
    events = []

    def first(row, col):
        events.append(("first", row, col))
        left.get = second
        return 2

    def second(row, col):
        events.append(("second", row, col))
        return 3

    left.get = first
    assert snapshot(molt_buffer.matmul(left, right)) == [[25]]
    assert events == [("first", 0, 0), ("second", 0, 1)]


def test_matmul_observes_class_get_and_result_set_replacement(monkeypatch):
    left = molt_buffer.new(1, 1, 2)
    right = molt_buffer.new(1, 1, 3)
    original_get = molt_buffer.Buffer2D.get
    original_set = molt_buffer.Buffer2D.set
    writes = []

    def get(self, row, col):
        return original_get(self, row, col) + 1

    def set(self, row, col, value):
        writes.append(value)
        return original_set(self, row, col, value + 10)

    monkeypatch.setattr(molt_buffer.Buffer2D, "get", get)
    monkeypatch.setattr(molt_buffer.Buffer2D, "set", set)
    result = molt_buffer.matmul(left, right)
    assert writes == [12]
    assert original_get(result, 0, 0) == 22


def test_matmul_preserves_custom_lookup_and_accessor_failure():
    events = []
    failure = RuntimeError("virtual cell failed")

    class ObservedBuffer(molt_buffer.Buffer2D):
        def __getattribute__(self, name):
            if name in ("rows", "cols", "get"):
                events.append(name)
            return super().__getattribute__(name)

        def get(self, row, col):
            raise failure

    left = ObservedBuffer(1, 1)
    with pytest.raises(RuntimeError) as raised:
        molt_buffer.matmul(left, molt_buffer.new(1, 1))
    assert raised.value is failure
    assert events == ["cols", "rows", "rows", "cols", "get"]
