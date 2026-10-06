#ifdef _MSC_VER
#define _Thread_local __declspec(thread)
#endif
#ifdef MOLT_PUBLIC_HEADER_PROBE
#include <molt/Python.h>
#else
#include <Python.h>
#endif
#include <stddef.h>
#include <string.h>

typedef struct {
    PyObject_HEAD
    PyObject *edge;
    long value;
} FactoryPayload;

static int finalizations;
static int frees;
static int payload_hash_calls;

static Py_hash_t payload_hash(PyObject *self) {
    ++payload_hash_calls;
    Py_hash_t hash = (Py_hash_t)((uintptr_t)self >> 4);
    return hash == -1 ? -2 : hash;
}

static int text_equals(PyObject *value, const char *expected) {
    const char *text = value == NULL ? NULL : PyUnicode_AsUTF8(value);
    return text != NULL && strcmp(text, expected) == 0;
}

static int payload_traverse(PyObject *self, visitproc visit, void *arg) {
    int status;
    PyObject *edge = ((FactoryPayload *)self)->edge;
    if (edge != NULL && (status = visit(edge, arg)) != 0) return status;
    return visit((PyObject *)Py_TYPE(self), arg);
}

static int payload_clear(PyObject *self) {
    Py_CLEAR(((FactoryPayload *)self)->edge);
    return 0;
}

static void payload_finalize(PyObject *self) {
    (void)self;
    ++finalizations;
}

static void payload_free(void *self) {
    ++frees;
    PyObject_GC_Del(self);
}

static PyObject *payload_repr(PyObject *self) {
    (void)self;
    return PyUnicode_FromString("factory-native");
}

static PyObject *payload_method(PyObject *self, PyObject *unused) {
    (void)unused;
    return PyLong_FromLong(((FactoryPayload *)self)->value + 1);
}

static PyObject *payload_get(PyObject *self, void *closure) {
    (void)closure;
    return PyLong_FromLong(((FactoryPayload *)self)->value);
}

static PyObject *payload_identity(PyObject *self, PyObject *value) {
    (void)self;
    Py_INCREF(value);
    return value;
}

static PyObject *payload_reject(PyObject *self, PyObject *value) {
    (void)self;
    (void)value;
    PyErr_SetString(PyExc_ValueError, "factory argument rejected");
    return NULL;
}

static PyObject *payload_keyword_identity(PyObject *self, PyObject *args, PyObject *kwargs) {
    (void)self;
    char *names[] = {"value", NULL};
    PyObject *value = NULL;
    if (!PyArg_ParseTupleAndKeywords(args, kwargs, "O", names, &value)) return NULL;
    Py_INCREF(value);
    return value;
}

static PyMethodDef payload_methods[] = {
    {"next_value", payload_method, METH_NOARGS, NULL},
    {"identity", payload_identity, METH_O, NULL},
    {"keyword_identity", (PyCFunction)(void (*)(void))payload_keyword_identity, METH_VARARGS | METH_KEYWORDS, NULL},
    {"reject", payload_reject, METH_O, NULL},
    {NULL, NULL, 0, NULL}
};
static PyMemberDef payload_members[] = {
    {"value", Py_T_LONG, offsetof(FactoryPayload, value), 0, NULL},
    {NULL, 0, 0, 0, NULL}
};
static PyGetSetDef payload_getsets[] = {
    {"current_value", payload_get, NULL, NULL, NULL},
    {NULL, NULL, NULL, NULL, NULL}
};

typedef struct {
    PyObject *edge;
    PyObject *type;
    int edge_visits;
    int type_visits;
} VisitWitness;

static int visit_payload(PyObject *value, void *arg) {
    VisitWitness *witness = (VisitWitness *)arg;
    if (value == witness->edge) ++witness->edge_visits;
    if (value == witness->type) ++witness->type_visits;
    return 0;
}

/* Real native objects cross every public container/call transport. A single
 * fixture is compiled against each header, including consuming failure paths. */
#include <stdarg.h>

static int parse_keywords_va(PyObject *args, PyObject *kwargs,
                             const char *format, char **names, ...) {
    va_list ap;
    va_start(ap, names);
    int result = PyArg_VaParseTupleAndKeywords(args, kwargs, format, names, ap);
    va_end(ap);
    return result;
}

typedef struct { PyObject *value; int cleanups; } ParseOwner;
typedef struct { PyObject *kwargs; int calls; } ParseOrder;

static int mutate_later_keyword(PyObject *value, void *address) {
    (void)value;
    ParseOrder *order = (ParseOrder *)address;
    ++order->calls;
    if (order->kwargs != NULL) {
        PyObject *next = PyLong_FromLong(42);
        if (next == NULL) return 0;
        int status = PyDict_SetItemString(order->kwargs, "last", next);
        Py_DECREF(next);
        if (status < 0) return 0;
    }
    return 1;
}

static int remove_unexpected_keyword(PyObject *value, void *address) {
    (void)value;
    ParseOrder *order = (ParseOrder *)address;
    ++order->calls;
    return PyDict_DelItemString(order->kwargs, "unknown") == 0;
}

static int retain_parse_object(PyObject *value, void *address) {
    ParseOwner *owner = (ParseOwner *)address;
    if (value == NULL) {
        Py_CLEAR(owner->value);
        ++owner->cleanups;
        return 1;
    }
    Py_INCREF(value);
    owner->value = value;
    return Py_CLEANUP_SUPPORTED;
}

typedef struct { int alive, retired, observed_live, observed_clear; } DrainState;
typedef struct { PyObject_HEAD DrainState *state; } DrainOwner;

static void retire_drain_owner(PyObject *object) {
    DrainState *state = ((DrainOwner *)object)->state;
    state->alive = 0;
    ++state->retired;
    PyTypeObject *type = Py_TYPE(object);
    PyObject_Free(object);
    Py_DECREF(type);
}

static PyObject *fail_drain_conversion(void *unused) {
    (void)unused;
    PyErr_SetString(PyExc_KeyError, "secondary drain error");
    return NULL;
}

static PyObject *observe_drain_conversion(void *address) {
    DrainState *state = (DrainState *)address;
    state->observed_live = state->alive;
    state->observed_clear = PyErr_Occurred() == NULL;
    Py_INCREF(Py_None);
    return Py_None;
}

static int check_keyword_and_parse_owners(PyObject *instance, PyObject *type,
                                         PyObject *module, PyObject *exception) {
    PyObject *args = NULL, *kwargs = NULL, *part = NULL, *result = NULL;
    PyObject *callable = NULL, *text = NULL;
    PyObject *a = NULL, *b = module, *c = NULL;
    ParseOwner owner = {NULL, 0};
    ParseOrder order = {NULL, 0};
    Py_buffer view = {0};
    char *encoded = NULL;
    char *names[] = {"first", "gap", "last", NULL};
    char *keyword_only[] = {"first", "last", NULL};
    char *positional_only[] = {"", "value", NULL};
    int status = 0;
#define PARSE_CHECK(condition, code) do { if (!(condition)) { status = (code); goto done; } } while (0)
    args = PyTuple_Pack(1, type);
    kwargs = PyDict_New();
    PARSE_CHECK(args != NULL && kwargs != NULL, 157);
    PARSE_CHECK(PyDict_SetItemString(kwargs, "last", exception) == 0, 158);
    PARSE_CHECK(PyArg_ParseTupleAndKeywords(args, kwargs, "O|O!O", names,
        &a, Py_TYPE(module), &b, &c) && a == type && b == module && c == exception, 159);
    a = c = NULL;
    PARSE_CHECK(parse_keywords_va(args, kwargs, "O|O!O", names,
        &a, Py_TYPE(module), &b, &c) && a == type && b == module && c == exception, 160);
    PARSE_CHECK(PyArg_ParseTupleAndKeywords(args, kwargs, "O|$O", keyword_only,
        &a, &c) && a == type && c == exception, 161);
    part = PyTuple_Pack(2, type, module);
    PARSE_CHECK(part != NULL, 162);
    PARSE_CHECK(!PyArg_ParseTupleAndKeywords(part, kwargs, "O|$O", keyword_only,
        &a, &c) && PyErr_ExceptionMatches(PyExc_TypeError), 163);
    PyErr_Clear();
    PyDict_Clear(kwargs);
    PARSE_CHECK(PyDict_SetItemString(kwargs, "first", module) == 0, 164);
    PARSE_CHECK(!PyArg_ParseTupleAndKeywords(args, kwargs, "O|O!O", names,
        &a, Py_TYPE(module), &b, &c) && PyErr_ExceptionMatches(PyExc_TypeError), 165);
    PyErr_Clear();
    PyDict_Clear(kwargs);
    PARSE_CHECK(PyDict_SetItemString(kwargs, "unknown", module) == 0, 166);
    PARSE_CHECK(!parse_keywords_va(args, kwargs, "O|O!O", names,
        &a, Py_TYPE(module), &b, &c) && PyErr_ExceptionMatches(PyExc_TypeError), 167);
    PyErr_Clear();
    PyDict_Clear(kwargs);
    PARSE_CHECK(PyDict_SetItem(kwargs, type, module) == 0, 168);
    PARSE_CHECK(!PyArg_ValidateKeywordArguments(kwargs)
        && PyErr_ExceptionMatches(PyExc_TypeError), 169);
    PyErr_Clear();
    PARSE_CHECK(!PyArg_ParseTupleAndKeywords(args, kwargs, "O|O!O", names,
        &a, Py_TYPE(module), &b, &c) && PyErr_ExceptionMatches(PyExc_TypeError), 170);
    PyErr_Clear();
    PyDict_Clear(kwargs);
    PARSE_CHECK(PyDict_SetItemString(kwargs, "value", module) == 0, 171);
    PARSE_CHECK(PyArg_ParseTupleAndKeywords(args, kwargs, "O|O", positional_only,
        &a, &b) && a == type && b == module, 172);
    Py_CLEAR(args);
    args = PyTuple_New(0);
    callable = PyObject_GetAttrString(instance, "keyword_identity");
    PARSE_CHECK(args != NULL && callable != NULL, 173);
    result = PyObject_Call(callable, args, kwargs);
    PARSE_CHECK(result == module && PyErr_Occurred() == NULL, 174);
    Py_CLEAR(result);
    Py_CLEAR(args);
    args = PyTuple_Pack(1, part);
    PARSE_CHECK(args != NULL && PyArg_ParseTuple(args, "(OO)", &a, &b)
        && a == type && b == module, 175);
    Py_CLEAR(args);
    args = PyTuple_Pack(2, type, module);
    PARSE_CHECK(args != NULL, 176);
    PARSE_CHECK(!PyArg_ParseTuple(args, "O&i", retain_parse_object, &owner, &(int){0})
        && PyErr_ExceptionMatches(PyExc_TypeError) && owner.value == NULL
        && owner.cleanups == 1, 177);
    PyErr_Clear();
    Py_CLEAR(args);
    text = PyBytes_FromStringAndSize("buffer", 6);
    PARSE_CHECK(text != NULL, 178);
    args = PyTuple_Pack(2, text, module);
    PARSE_CHECK(args != NULL, 179);
    PARSE_CHECK(!PyArg_ParseTuple(args, "y*i", &view, &(int){0})
        && PyErr_ExceptionMatches(PyExc_TypeError) && view.obj == NULL, 180);
    PyErr_Clear();
    Py_CLEAR(args);
    Py_CLEAR(text);
    text = PyUnicode_FromString("encoded");
    PARSE_CHECK(text != NULL, 181);
    args = PyTuple_Pack(2, text, module);
    PARSE_CHECK(args != NULL, 182);
    PARSE_CHECK(!PyArg_ParseTuple(args, "esi", "utf-8", &encoded, &(int){0})
        && PyErr_ExceptionMatches(PyExc_TypeError) && encoded == NULL, 183);
    PyErr_Clear();
    Py_CLEAR(args);
    args = PyTuple_Pack(1, type);
    PyDict_Clear(kwargs);
    PARSE_CHECK(args != NULL && PyDict_SetItemString(kwargs, "last", type) == 0, 197);
    order.kwargs = kwargs;
    {
        int later = 0;
        PARSE_CHECK(PyArg_ParseTupleAndKeywords(args, kwargs, "O&|Oi", names,
            mutate_later_keyword, &order, &b, &later) && order.calls == 1 && later == 42, 198);
    }
    order.kwargs = NULL;
    order.calls = 0;
    PyDict_Clear(kwargs);
    PARSE_CHECK(!PyArg_ParseTuple(args, "O&i", mutate_later_keyword, &order, &(int){0})
        && order.calls == 0 && PyErr_ExceptionMatches(PyExc_TypeError), 206);
    PyErr_Clear();
    PARSE_CHECK(!PyArg_ParseTupleAndKeywords(args, kwargs, "O&O", keyword_only,
        mutate_later_keyword, &order, &c) && order.calls == 1
        && PyErr_ExceptionMatches(PyExc_TypeError), 199);
    PyErr_Clear();
    order.calls = 0;
    PARSE_CHECK(PyDict_SetItemString(kwargs, "unknown", module) == 0, 200);
    PARSE_CHECK(!PyArg_ParseTupleAndKeywords(args, kwargs, "O&|OO", names,
        mutate_later_keyword, &order, &b, &c) && order.calls == 1
        && PyErr_ExceptionMatches(PyExc_TypeError), 201);
    PyErr_Clear();
    order.calls = 0;
    PyDict_Clear(kwargs);
    PARSE_CHECK(PyDict_SetItemString(kwargs, "first", module) == 0, 202);
    PARSE_CHECK(!PyArg_ParseTupleAndKeywords(args, kwargs, "O&|OO", names,
        mutate_later_keyword, &order, &b, &c) && order.calls == 1
        && PyErr_ExceptionMatches(PyExc_TypeError), 203);
    PyErr_Clear();
    order.kwargs = kwargs;
    order.calls = 0;
    PyDict_Clear(kwargs);
    PARSE_CHECK(PyDict_SetItemString(kwargs, "unknown", module) == 0, 208);
    PARSE_CHECK(!PyArg_ParseTupleAndKeywords(args, kwargs, "O&|O", keyword_only,
        remove_unexpected_keyword, &order, &c) && order.calls == 1
        && PyDict_Size(kwargs) == 0 && PyErr_ExceptionMatches(PyExc_TypeError), 209);
    PyErr_Clear();
done:
    {
        PyObject *error = PyErr_GetRaisedException();
        if (view.obj != NULL) PyBuffer_Release(&view);
        PyMem_Free(encoded);
        Py_XDECREF(owner.value);
        Py_XDECREF(text);
        Py_XDECREF(result);
        Py_XDECREF(callable);
        Py_XDECREF(part);
        Py_XDECREF(kwargs);
        Py_XDECREF(args);
        PyErr_SetRaisedException(error);
    }
#undef PARSE_CHECK
    return status;
}

static int check_unchecked_stores(PyObject *value, int as_list) {
    Py_ssize_t refs = Py_REFCNT(value);
    PyObject *sequence = as_list ? PyList_New(1) : PyTuple_New(1);
    if (sequence == NULL) return 184;
    Py_INCREF(value);
    if (as_list) PyList_SET_ITEM(sequence, 0, value);
    else PyTuple_SET_ITEM(sequence, 0, value);
    int good = (as_list ? PyList_GET_ITEM(sequence, 0) : PyTuple_GET_ITEM(sequence, 0)) == value
        && PyErr_Occurred() == NULL;
    if (good) {
        /* Clearing an occupied slot preserves its stolen physical reference. */
        if (as_list) PyList_SET_ITEM(sequence, 0, NULL);
        else PyTuple_SET_ITEM(sequence, 0, NULL);
        good = (as_list ? PyList_GET_ITEM(sequence, 0) : PyTuple_GET_ITEM(sequence, 0)) == NULL
            && PyErr_Occurred() == NULL;
    }
    PyObject *error = PyErr_GetRaisedException();
    Py_DECREF(sequence);
    good = good && Py_REFCNT(value) == refs + 1;
    /* Reclaim the deliberately displaced reference even if a contract failed. */
    while (Py_REFCNT(value) > refs) Py_DECREF(value);
    PyErr_SetRaisedException(error);
    return good ? 0 : 185;
}

static int check_container_call_authority(PyObject *instance,
                                          PyObject *type, PyObject *module, PyObject **drain_type) {
    PyObject *tuple = NULL, *list = NULL, *mapping = NULL, *set = NULL;
    PyObject *part = NULL, *result = NULL, *exception = NULL, *key = NULL;
    PyObject *method = NULL, *name = NULL;
    PyObject *items[3];
    int status = 0;
#define CHECK(condition, code) do { if (!(condition)) { status = (code); goto done; } } while (0)
    PyErr_SetString(PyExc_ValueError, "physical container element");
    exception = PyErr_GetRaisedException();
    CHECK(exception != NULL && PyErr_Occurred() == NULL, 100);
    status = check_keyword_and_parse_owners(instance, type, module, exception);
    if (status != 0) goto done;
    {
        DrainState state = {1, 0, 0, 0};
        PyType_Slot drain_slots[] = {{Py_tp_dealloc, (void *)retire_drain_owner}, {0, NULL}};
        PyType_Spec drain_spec = {"factory.DrainOwner", sizeof(DrainOwner), 0, 0, drain_slots};
        *drain_type = PyType_FromSpec(&drain_spec);
        CHECK(*drain_type != NULL, 210);
        DrainOwner *owner = (DrainOwner *)PyType_GenericAlloc((PyTypeObject *)*drain_type, 0);
        CHECK(owner != NULL, 211);
        owner->state = &state;
        result = Py_BuildValue("ONO&O&", NULL, (PyObject *)owner,
            fail_drain_conversion, NULL, observe_drain_conversion, &state);
        CHECK(result == NULL && PyErr_ExceptionMatches(PyExc_SystemError)
            && state.observed_live && state.observed_clear
            && state.retired == 1 && !state.alive, 186);
        PyErr_Clear();
    }
    items[0] = type;
    items[1] = module;
    items[2] = exception;
    key = PyUnicode_FromString("element");
    name = PyUnicode_FromString("identity");
    method = PyObject_GetAttrString(instance, "identity");
    CHECK(key != NULL && name != NULL && method != NULL, 101);
    for (int frozen = 0; frozen < 2; ++frozen) {
        set = frozen ? PyFrozenSet_New(NULL) : PySet_New(NULL);
        CHECK(set != NULL && PySet_Size(set) == 0, 191);
        Py_CLEAR(set);
        set = frozen ? PyFrozenSet_New(Py_None) : PySet_New(Py_None);
        CHECK(set == NULL && PyErr_ExceptionMatches(PyExc_TypeError), 192);
        PyErr_Clear();
        part = PyFloat_FromDouble(0.0);
        CHECK(part != NULL, 193);
        set = frozen ? PyFrozenSet_New(part) : PySet_New(part);
        CHECK(set == NULL && PyErr_ExceptionMatches(PyExc_TypeError), 194);
        PyErr_Clear();
        Py_CLEAR(part);
    }
    mapping = PyDict_New();
    CHECK(mapping != NULL && PyDict_SetItem(mapping, instance, module) == 0, 195);
    payload_hash_calls = 0;
    CHECK(PyDict_Pop(mapping, instance, &result) == 1 && result == module
        && payload_hash_calls == 1, 196);
    Py_CLEAR(result);
    CHECK(PyDict_SetItem(mapping, key, Py_None) == 0
        && PyDict_Pop(mapping, key, &result) == 1 && result == Py_None, 204);
    Py_CLEAR(result);
    CHECK(PyDict_Pop(mapping, key, &result) == 0 && result == NULL, 205);
    Py_CLEAR(mapping);
    part = PyObject_GetAttrString(instance, "next_value");
    CHECK(part != NULL, 187);
    result = PyObject_CallFunction(part, " ,:\t");
    CHECK(result != NULL && PyErr_Occurred() == NULL, 188);
    Py_CLEAR(result);
    Py_CLEAR(part);
    result = PyObject_CallMethod(instance, "next_value", " ");
    CHECK(result != NULL && PyErr_Occurred() == NULL, 189);
    Py_CLEAR(result);
    result = PyObject_CallFunction(method, "O", Py_None);
    CHECK(result == Py_None, 190);
    Py_CLEAR(result);

    tuple = PyTuple_FromArray(items, 3);
    CHECK(tuple != NULL && PyTuple_Size(tuple) == 3 && PyErr_Occurred() == NULL, 102);
    for (int i = 0; i < 3; ++i) {
        CHECK(PyTuple_GetItem(tuple, i) == items[i]
            && ((PyTupleObject *)tuple)->ob_item[i] == items[i], 103);
    }
    part = PyTuple_GetSlice(tuple, 0, 3);
    CHECK(part != NULL && PyTuple_GetItem(part, 1) == module, 104);
    Py_CLEAR(part);
    list = PySequence_List(tuple);
    CHECK(list != NULL && PyList_Size(list) == 3, 105);
    part = PyList_AsTuple(list);
    CHECK(part != NULL && PyTuple_GetItem(part, 2) == exception, 106);
    Py_CLEAR(part);
    part = PySequence_GetSlice(list, 0, 3);
    CHECK(part != NULL && PyList_GetItem(part, 0) == type, 107);
    Py_CLEAR(part);
    CHECK(PySequence_SetSlice(list, 0, 3, tuple) == 0, 108);
    Py_CLEAR(list);
    Py_CLEAR(tuple);

    tuple = Py_BuildValue("(OOO)", type, module, exception);
    CHECK(tuple != NULL && PyTuple_GetItem(tuple, 0) == type
        && PyTuple_GetItem(tuple, 1) == module && PyTuple_GetItem(tuple, 2) == exception, 109);
    {
        PyObject *a = NULL, *b = NULL, *c = NULL;
        CHECK(PyArg_UnpackTuple(tuple, "physical", 3, 3, &a, &b, &c)
            && a == type && b == module && c == exception, 110);
        CHECK(PyArg_ParseTuple(tuple, "OOO", &a, &b, &c)
            && a == type && b == module && c == exception, 111);
    }
    Py_CLEAR(tuple);

    for (int i = 0; i < 3; ++i) {
        PyObject *value = items[i];
        Py_ssize_t refs = Py_REFCNT(value);
        status = check_unchecked_stores(value, 0);
        if (status != 0) goto done;
        status = check_unchecked_stores(value, 1);
        if (status != 0) goto done;
        tuple = PyTuple_Pack(1, value);
        CHECK(tuple != NULL && PyTuple_GetItem(tuple, 0) == value, 112);
        list = PyList_New(1);
        CHECK(list != NULL, 113);
        Py_INCREF(value);
        CHECK(PyList_SetItem(list, 0, value) == 0, 114);
        CHECK(PyList_GetItem(list, 0) == value
            && ((PyListObject *)list)->ob_item[0] == value, 115);
        result = PyList_GetItemRef(list, 0);
        CHECK(result == value, 116);
        Py_CLEAR(result);
        CHECK(PyList_Append(list, value) == 0 && PyList_GetItem(list, 1) == value, 117);
        result = PySequence_GetItem(list, -1);
        CHECK(result == value, 118);
        Py_CLEAR(result);
        CHECK(PySequence_SetItem(list, 0, value) == 0, 119);
        Py_CLEAR(list);

        mapping = PyDict_New();
        CHECK(mapping != NULL && PyDict_SetItem(mapping, key, value) == 0, 120);
        CHECK(PyDict_GetItem(mapping, key) == value, 121);
        CHECK(PyDict_GetItemRef(mapping, key, &result) == 1 && result == value, 122);
        Py_CLEAR(result);
        result = PyObject_GetItem(mapping, key);
        CHECK(result == value, 123);
        Py_CLEAR(result);
        CHECK(PyMapping_SetItemString(mapping, "element", value) == 0, 124);
        result = PyMapping_GetItemString(mapping, "element");
        CHECK(result == value, 125);
        Py_CLEAR(result);
        CHECK(PyDict_SetItem(mapping, value, module) == 0
            && PyDict_GetItem(mapping, value) == module, 126);
        CHECK(PyDict_Pop(mapping, value, &result) == 1 && result == module, 127);
        Py_CLEAR(result);
        CHECK(PyMapping_DelItemString(mapping, "element") == 0 && PyDict_Size(mapping) == 0, 128);
        Py_CLEAR(mapping);

        set = PySet_New(NULL);
        CHECK(set != NULL && PySet_Add(set, value) == 0 && PySet_Contains(set, value) == 1, 129);
        result = PySet_Pop(set);
        CHECK(result == value && PySet_Size(set) == 0, 130);
        Py_CLEAR(result);
        Py_CLEAR(set);
        set = PyFrozenSet_New(tuple);
        CHECK(set != NULL && PySet_Size(set) == 1 && PySet_Contains(set, value) == 1, 207);
        Py_CLEAR(set);

        result = PyObject_Call(method, tuple, NULL);
        CHECK(result == value, 131);
        Py_CLEAR(result);
        result = PyObject_CallFunctionObjArgs(method, value, NULL);
        CHECK(result == value, 132);
        Py_CLEAR(result);
        result = PyObject_CallMethodObjArgs(instance, name, value, NULL);
        CHECK(result == value, 133);
        Py_CLEAR(result);
        result = PyObject_CallFunction(method, "O", value);
        CHECK(result == value, 134);
        Py_CLEAR(result);
        result = PyObject_CallMethod(instance, "identity", "O", value);
        CHECK(result == value, 135);
        Py_CLEAR(result);
        result = PyObject_CallOneArg(method, value);
        CHECK(result == value, 136);
        Py_CLEAR(result);
        result = PyObject_Vectorcall(method, &value, 1, NULL);
        CHECK(result == value, 137);
        Py_CLEAR(result);
        result = PyObject_VectorcallDict(method, &value, 1, NULL);
        CHECK(result == value, 138);
        Py_CLEAR(result);
        {
            PyObject *args[] = {instance, value};
            result = PyObject_VectorcallMethod(name, args, 2, NULL);
            CHECK(result == value, 139);
            Py_CLEAR(result);
        }
        {
            PyObject *parsed = NULL;
            CHECK(PyArg_Parse(value, "O", &parsed) && parsed == value, 140);
        }
        Py_CLEAR(tuple);
        CHECK(Py_REFCNT(value) == refs && PyErr_Occurred() == NULL, 141);

        result = PyTuple_Pack(2, value, NULL);
        CHECK(result == NULL && PyErr_Occurred() != NULL, 142);
        PyErr_Clear();
        CHECK(Py_REFCNT(value) == refs, 143);
        tuple = PyTuple_New(0);
        CHECK(tuple != NULL, 144);
        Py_INCREF(value);
        CHECK(PyTuple_SetItem(tuple, 0, value) < 0 && PyErr_Occurred() != NULL, 145);
        PyErr_Clear();
        Py_CLEAR(tuple);
        CHECK(Py_REFCNT(value) == refs, 146);
        list = PyList_New(0);
        CHECK(list != NULL, 147);
        Py_INCREF(value);
        CHECK(PyList_SetItem(list, 0, value) < 0 && PyErr_Occurred() != NULL, 148);
        PyErr_Clear();
        Py_CLEAR(list);
        CHECK(Py_REFCNT(value) == refs, 149);
        Py_INCREF(value);
        result = Py_BuildValue("(ONO)", value, value, NULL);
        CHECK(result == NULL && PyErr_Occurred() != NULL, 150);
        PyErr_Clear();
        CHECK(Py_REFCNT(value) == refs, 151);
        Py_INCREF(value);
        result = Py_BuildValue("ON", NULL, value);
        CHECK(result == NULL && PyErr_Occurred() != NULL, 152);
        PyErr_Clear();
        CHECK(Py_REFCNT(value) == refs, 153);
        result = PyObject_CallMethod(instance, "reject", "O", value);
        CHECK(result == NULL && PyErr_ExceptionMatches(PyExc_ValueError), 154);
        PyErr_Clear();
        CHECK(Py_REFCNT(value) == refs, 155);
    }
    CHECK(PyErr_Occurred() == NULL, 156);
done:
    {
        PyObject *error = PyErr_GetRaisedException();
        Py_XDECREF(result);
        Py_XDECREF(part);
        Py_XDECREF(set);
        Py_XDECREF(mapping);
        Py_XDECREF(list);
        Py_XDECREF(tuple);
        Py_XDECREF(method);
        Py_XDECREF(name);
        Py_XDECREF(key);
        Py_XDECREF(exception);
        PyErr_SetRaisedException(error);
    }
#undef CHECK
    return status;
}

/* CPython 3.12 Objects/abstract.c class-info protocols and Objects/object.c
 * slot/hash/richcompare contracts. Both Molt header consumers execute this
 * same source; callbacks supply independently specified values and errors. */
static PyObject *observation_result;
static PyObject *observation_failure;
static int observation_attributes, observation_truth_calls, observation_truth;
static int observation_left_calls, observation_right_calls, observation_opcode;
static int observation_compare_mode, observation_check_calls, observation_check_fails;
static int observation_conversion_mode, observation_length_calls;
static int observation_buffer_acquires, observation_buffer_releases;
static int observation_buffer_mode, observation_release_error;
typedef PyObject *(*BufferConstructorProbe)(PyObject *, int);
static PyObject *observation_dir_result;
static int observation_sequence_names;

static int observation_list_contains(PyObject *list, const char *text) {
    for (Py_ssize_t i = 0; i < PyList_Size(list); ++i)
        if (text_equals(PyList_GetItem(list, i), text)) return 1;
    return 0;
}

static PyObject *observation_raise(void) {
    PyErr_SetObject(PyExc_LookupError, observation_failure);
    return NULL;
}

static int observation_error_is_original(void) {
    PyObject *error = PyErr_GetRaisedException();
    int same = error == observation_failure;
    Py_XDECREF(error);
    return same;
}

static PyObject *observation_getattr(PyObject *self, PyObject *name) {
    ++observation_attributes;
    if (text_equals(name, "__call__")) {
        Py_INCREF(Py_True);
        return Py_True;
    }
    if (text_equals(name, "__class__") || text_equals(name, "__bases__")
        || text_equals(name, "__hash__") || text_equals(name, "__lt__"))
        return observation_raise();
    return PyObject_GenericGetAttr(self, name);
}

static int observation_bool(PyObject *self) {
    (void)self;
    ++observation_truth_calls;
    if (observation_truth < 0) observation_raise();
    return observation_truth;
}

static Py_hash_t observation_hash(PyObject *self) {
    (void)self;
    return 867;
}

static PyObject *observation_call(PyObject *self, PyObject *args, PyObject *kwargs) {
    (void)self; (void)args; (void)kwargs;
    Py_INCREF(Py_None);
    return Py_None;
}

static PyObject *observation_compare(PyObject *self, PyObject *other, int op) {
    (void)self; (void)other;
    ++observation_left_calls;
    observation_opcode = op;
    if (observation_compare_mode == 2) return observation_raise();
    PyObject *result = observation_compare_mode == 1 ? Py_NotImplemented : observation_result;
    Py_INCREF(result);
    return result;
}

static PyObject *observation_reflected(PyObject *self, PyObject *other, int op) {
    --observation_left_calls;
    ++observation_right_calls;
    return observation_compare(self, other, op);
}

static PyObject *observation_classcheck(PyObject *self, PyObject *candidate) {
    (void)self; (void)candidate;
    ++observation_check_calls;
    if (observation_check_fails) return observation_raise();
    Py_INCREF(observation_result);
    return observation_result;
}

static PyMethodDef observation_meta_methods[] = {
    {"__instancecheck__", observation_classcheck, METH_O, NULL},
    {"__subclasscheck__", observation_classcheck, METH_O, NULL},
    {NULL, NULL, 0, NULL}
};

static PyObject *observation_string(PyObject *self) {
    (void)self;
    if (observation_conversion_mode == 1) return observation_raise();
    if (observation_conversion_mode == 2) return PyLong_FromLong(17);
    return PyUnicode_FromString("native-string");
}

static Py_ssize_t observation_length(PyObject *self) {
    (void)self;
    ++observation_length_calls;
    if (observation_conversion_mode == 1) { observation_raise(); return -1; }
    return 7;
}

static PyObject *observation_bytes(PyObject *self, PyObject *unused) {
    (void)self; (void)unused;
    if (observation_conversion_mode == 1) return observation_raise();
    if (observation_conversion_mode == 2) return PyLong_FromLong(17);
    return PyBytes_FromString("native-bytes");
}

static PyObject *observation_format(PyObject *self, PyObject *spec) {
    (void)self;
    if (observation_conversion_mode == 1) return observation_raise();
    if (observation_conversion_mode == 2) return PyLong_FromLong(17);
    if (!text_equals(spec, "") && !text_equals(spec, ".native")) return observation_raise();
    return PyUnicode_FromString("native-format");
}

static PyObject *observation_dir(PyObject *self, PyObject *unused) {
    (void)self; (void)unused;
    if (observation_conversion_mode == 1) return observation_raise();
    if (observation_dir_result != NULL) {
        Py_INCREF(observation_dir_result);
        return observation_dir_result;
    }
    return Py_BuildValue("(ss)", "z-native", "a-native");
}

static PyMethodDef observation_methods[] = {
    {"__bytes__", observation_bytes, METH_NOARGS, NULL},
    {"__format__", observation_format, METH_O, NULL},
    {"__dir__", observation_dir, METH_NOARGS, NULL},
    {NULL, NULL, 0, NULL}
};

static int observation_buffer(PyObject *self, Py_buffer *view, int flags) {
    static unsigned char data[] = {65, 0, 66, 0, 67};
    static Py_ssize_t shape = 3, stride = 2;
    if (observation_conversion_mode == 1) { observation_raise(); return -1; }
    if (flags != (observation_buffer_mode == 5 ? PyBUF_SIMPLE : PyBUF_FULL_RO)) {
        PyErr_SetString(PyExc_BufferError, "unexpected export flags"); return -1;
    }
    /* Exercise the actual header FillInfo against linked caller storage. */
    if (PyBuffer_FillInfo(view, self, data, 3, 1, flags) < 0) return -1;
    ++observation_buffer_acquires;
    if (observation_buffer_mode == 5) return 0; /* Simple contiguous A, NUL, B. */
    view->shape = &shape;
    view->strides = &stride;
    if (observation_buffer_mode == 2) view->format = "T{B:field_with_a_long_name:}";
    if (observation_buffer_mode == 3) view->len = 4; /* Invalid geometry, after acquisition. */
    return 0;
}

static void observation_releasebuffer(PyObject *self, Py_buffer *view) {
    (void)self; (void)view;
    ++observation_buffer_releases;
    if (observation_release_error) PyErr_SetString(PyExc_TypeError, "release must not replace conversion error");
}

static PyObject *observation_buffer_index(PyObject *self) {
    (void)self;
    if (observation_buffer_mode == 4) return PyLong_FromLong(2);
    if (observation_buffer_mode == 5) return observation_raise();
    PyErr_SetString(PyExc_TypeError, "buffer is not a count");
    return NULL;
}

static Py_ssize_t observation_sequence_length(PyObject *self) { (void)self; return 2; }
static PyObject *observation_sequence_item(PyObject *self, Py_ssize_t index) {
    (void)self;
    if (observation_conversion_mode == 1) return observation_raise();
    if (index >= 0 && index < 2) {
        if (observation_sequence_names) return PyUnicode_FromString(index == 0 ? "z-native" : "a-native");
        return PyLong_FromLong(index == 0 ? 65 : 255);
    }
    PyErr_SetNone(PyExc_IndexError);
    return NULL;
}

/* Membership alone carries no PyObject layout. Deliberately opaque storage
 * proves rejection before a header read in both transports, including identity
 * shortcuts and explicit PyTypeObject operands. */
#ifndef MOLT_PUBLIC_HEADER_PROBE
extern int molt_c_heap_register(uintptr_t pointer);
extern int molt_c_heap_unregister(uintptr_t pointer);
#endif
static int check_private_observation_admission(PyObject *valid) {
    union { PyObject alignment; unsigned char bytes[64]; } storage;
    memset(storage.bytes, 0xa5, sizeof(storage.bytes));
    PyObject *private_object = (PyObject *)&storage;
    int status = 0;
    if (molt_c_heap_register((uintptr_t)private_object) != 0) return 260;
#define PRIVATE_EXPECT(expression, expected, code) do { \
    if ((expression) != (expected) || !PyErr_ExceptionMatches(PyExc_TypeError)) { \
        status = (code); goto done; \
    } \
    PyErr_Clear(); \
} while (0)
    PRIVATE_EXPECT(PyType_Check(private_object), 0, 261);
    PRIVATE_EXPECT(PyType_CheckExact(private_object), 0, 262);
    PRIVATE_EXPECT(PyType_IsSubtype((PyTypeObject *)private_object, &PyLong_Type), 0, 263);
    PRIVATE_EXPECT(PyType_IsSubtype(&PyLong_Type, (PyTypeObject *)private_object), 0, 264);
    PRIVATE_EXPECT(PyObject_Type(private_object), NULL, 265);
    PRIVATE_EXPECT(PyObject_TypeCheck(private_object, &PyLong_Type), 0, 266);
    PRIVATE_EXPECT(PyObject_TypeCheck(valid, (PyTypeObject *)private_object), 0, 267);
    PRIVATE_EXPECT(PyObject_IsInstance(private_object, (PyObject *)&PyLong_Type), -1, 268);
    PRIVATE_EXPECT(PyObject_IsInstance(valid, private_object), -1, 269);
    PRIVATE_EXPECT(PyObject_IsSubclass(private_object, (PyObject *)&PyLong_Type), -1, 270);
    PRIVATE_EXPECT(PyObject_IsSubclass((PyObject *)&PyLong_Type, private_object), -1, 271);
    PRIVATE_EXPECT(PyCallable_Check(private_object), 0, 272);
    PRIVATE_EXPECT(PyObject_Hash(private_object), -1, 273);
    PRIVATE_EXPECT(PyObject_HashNotImplemented(private_object), -1, 274);
    PRIVATE_EXPECT(PyObject_IsTrue(private_object), -1, 275);
    PRIVATE_EXPECT(PyObject_Not(private_object), -1, 276);
    PRIVATE_EXPECT(PyObject_RichCompare(private_object, valid, Py_EQ), NULL, 277);
    PRIVATE_EXPECT(PyObject_RichCompare(valid, private_object, Py_EQ), NULL, 278);
    PRIVATE_EXPECT(PyObject_RichCompareBool(private_object, private_object, Py_EQ), -1, 279);
    PRIVATE_EXPECT(PyObject_RichCompareBool(valid, private_object, Py_NE), -1, 280);
    PRIVATE_EXPECT(PyObject_Str(private_object), NULL, 282);
    PRIVATE_EXPECT(PyObject_Repr(private_object), NULL, 283);
    PRIVATE_EXPECT(PyObject_Length(private_object), -1, 284);
    PRIVATE_EXPECT(PyObject_Size(private_object), -1, 285);
    PRIVATE_EXPECT(PyObject_Bytes(private_object), NULL, 286);
    PRIVATE_EXPECT(PyObject_Format(private_object, NULL), NULL, 287);
    PRIVATE_EXPECT(PyObject_Format(valid, private_object), NULL, 288);
    PRIVATE_EXPECT(PyObject_Dir(private_object), NULL, 289);
    for (size_t i = 0; i < sizeof(storage.bytes); ++i) {
        if (storage.bytes[i] != 0xa5) { status = 281; goto done; }
    }
done:
    (void)molt_c_heap_unregister((uintptr_t)private_object);
#undef PRIVATE_EXPECT
    return status;
}

/* Both header facades run this exporter through C conversion and the actual
 * runtime bytes/bytearray constructor callback supplied by the Rust wrapper. */
static int check_buffer_authority(PyObject *exporter, BufferConstructorProbe constructor) {
    unsigned char data[2] = {11, 22};
    struct { size_t before; Py_buffer view; size_t after; } guarded = {0};
    PyObject *views[65] = {0};
    PyObject *result = NULL;
    Py_buffer *first = NULL;
    Py_ssize_t refs = Py_REFCNT(exporter);
    int status = 0, before;
#define BUFFER_REQUIRE(condition, code) do { if (!(condition)) { status = (code); goto done; } } while (0)
    guarded.before = (size_t)0x13579bdf;
    guarded.after = (size_t)0x2468ace0;
    BUFFER_REQUIRE(PyBuffer_FillInfo(&guarded.view, NULL, data, 2, 1, PyBUF_FULL_RO) == 0, 401);
    BUFFER_REQUIRE(guarded.before == (size_t)0x13579bdf && guarded.after == (size_t)0x2468ace0
        && guarded.view.shape == &guarded.view.len && guarded.view.strides == &guarded.view.itemsize
        && guarded.view.internal == NULL && strcmp(guarded.view.format, "B") == 0, 402);
    views[0] = PyMemoryView_FromBuffer(&guarded.view);
    BUFFER_REQUIRE(views[0] != NULL && PyMemoryView_GET_BASE(views[0]) == NULL, 421);
    PyBuffer_Release(&guarded.view);
    first = PyMemoryView_GET_BUFFER(views[0]);
    BUFFER_REQUIRE(first != NULL && first->len == 2 && first->shape[0] == 2
        && first->strides[0] == 1 && first->shape != &guarded.view.len, 422);
    Py_CLEAR(views[0]);
    /* More than the retired TLS cache capacity; borrowed view identity belongs
     * to the object and cannot be evicted by observing unrelated memoryviews. */
    for (int i = 0; i < 65; ++i) {
        views[i] = PyMemoryView_FromMemory((char *)data, i == 0 ? 1 : 2, PyBUF_READ);
        BUFFER_REQUIRE(views[i] != NULL && PyMemoryView_Check(views[i]), 403);
        Py_buffer *current = PyMemoryView_GET_BUFFER(views[i]);
        BUFFER_REQUIRE(current != NULL && current->len == (i == 0 ? 1 : 2), 404);
        if (i == 0) first = current;
    }
    BUFFER_REQUIRE(first == PyMemoryView_GET_BUFFER(views[0]) && first->len == 1, 405);
    for (int i = 0; i < 65; ++i) Py_CLEAR(views[i]);
    before = observation_buffer_acquires;
    observation_raise();
    BUFFER_REQUIRE(PyObject_CheckBuffer(exporter) == 1 && observation_buffer_acquires == before
        && observation_error_is_original(), 406);
    for (int kind = 0; kind < 2; ++kind) {
        for (int format = 0; format < 2; ++format) {
            observation_buffer_mode = format ? 2 : 0;
            before = observation_buffer_acquires;
            result = constructor(exporter, kind);
            BUFFER_REQUIRE(result != NULL, 407);
            BUFFER_REQUIRE((kind == 0 ? PyBytes_Size(result) : PyByteArray_Size(result)) == 3
                && memcmp(kind == 0 ? PyBytes_AsString(result) : PyByteArray_AsString(result), "ABC", 3) == 0
                && observation_buffer_acquires == before + 1
                && observation_buffer_releases == observation_buffer_acquires
                && Py_REFCNT(exporter) == refs, 408);
            Py_CLEAR(result);
        }
    }
    /* The failed index probe is deliberately replaced by PyBUF_SIMPLE.
     * Both public headers exercise flags, exact exporter custody and release. */
    observation_buffer_mode = 5;
    for (int kind = 0; kind < 2; ++kind) {
        result = kind ? PyByteArray_FromStringAndSize("XA\0BY", 5)
                      : PyBytes_FromStringAndSize("XA\0BY", 5);
        BUFFER_REQUIRE(result != NULL, 470);
        before = observation_buffer_acquires;
        BUFFER_REQUIRE(PySequence_Contains(result, exporter) == 1
            && !PyErr_Occurred() && observation_buffer_acquires == before + 1
            && observation_buffer_releases == observation_buffer_acquires
            && Py_REFCNT(exporter) == refs, 471);
        observation_conversion_mode = 1;
        BUFFER_REQUIRE(PySequence_Contains(result, exporter) == -1
            && observation_error_is_original(), 472);
        observation_conversion_mode = 0;
        Py_CLEAR(result);
    }
    observation_buffer_mode = 2;
    result = PyObject_Bytes(exporter);
    BUFFER_REQUIRE(result != NULL && PyBytes_Size(result) == 3
        && memcmp(PyBytes_AsString(result), "ABC", 3) == 0, 409);
    Py_CLEAR(result);
    observation_buffer_mode = 4;
    before = observation_buffer_acquires;
    result = constructor(exporter, 0);
    BUFFER_REQUIRE(result != NULL && PyBytes_Size(result) == 2
        && PyBytes_AsString(result)[0] == 0 && PyBytes_AsString(result)[1] == 0
        && observation_buffer_acquires == before, 410);
    Py_CLEAR(result);
    result = PyObject_Bytes(exporter); /* C conversion never uses the count. */
    BUFFER_REQUIRE(result != NULL && PyBytes_Size(result) == 3
        && memcmp(PyBytes_AsString(result), "ABC", 3) == 0, 411);
    Py_CLEAR(result);
    observation_buffer_mode = 3;
    observation_release_error = 1;
    BUFFER_REQUIRE(constructor(exporter, 0) == NULL && PyErr_ExceptionMatches(PyExc_BufferError), 412);
    PyErr_Clear();
    BUFFER_REQUIRE(PyObject_Bytes(exporter) == NULL && PyErr_ExceptionMatches(PyExc_BufferError), 413);
    PyErr_Clear();
    observation_release_error = observation_buffer_mode = 0;

    /* Runtime-created views return through the same Check/GET_BUFFER projection.
     * C-created views must provide Python methods, sequence and buffer semantics. */
    views[0] = constructor(exporter, 2);
    BUFFER_REQUIRE(views[0] != NULL && PyMemoryView_Check(views[0])
        && PyMemoryView_GET_BASE(views[0]) == exporter, 423);
    result = PyObject_GetAttrString(views[0], "obj");
    BUFFER_REQUIRE(result == exporter, 424);
    Py_CLEAR(result);
    BUFFER_REQUIRE(PyObject_Size(views[0]) == 3 && PyObject_CheckBuffer(views[0]), 425);
    result = PySequence_GetItem(views[0], 1);
    BUFFER_REQUIRE(result != NULL && PyLong_AsLong(result) == 'B', 426);
    Py_CLEAR(result);
    result = PyObject_CallMethod(views[0], "tobytes", NULL);
    BUFFER_REQUIRE(result != NULL && PyBytes_Size(result) == 3
        && memcmp(PyBytes_AsString(result), "ABC", 3) == 0, 427);
    Py_CLEAR(result);
    BUFFER_REQUIRE(PyObject_GetBuffer(views[0], &guarded.view, PyBUF_FULL_RO) == 0, 428);
    result = PyObject_CallMethod(views[0], "release", NULL);
    BUFFER_REQUIRE(result == NULL && PyErr_ExceptionMatches(PyExc_BufferError), 429);
    PyErr_Clear();
    PyBuffer_Release(&guarded.view);
    views[1] = PyObject_CallMethod(views[0], "toreadonly", NULL);
    BUFFER_REQUIRE(views[1] != NULL && PyMemoryView_Check(views[1])
        && PyMemoryView_GET_BUFFER(views[1])->readonly, 430);
    result = PyObject_CallMethod(views[0], "release", NULL);
    BUFFER_REQUIRE(result != NULL && PyMemoryView_Check(views[0])
        && PyMemoryView_GET_BASE(views[0]) == NULL, 431);
    Py_CLEAR(result);
    Py_CLEAR(views[0]);
    result = PyObject_CallMethod(views[1], "tobytes", NULL);
    BUFFER_REQUIRE(result != NULL && PyBytes_Size(result) == 3, 432);
    Py_CLEAR(result);
    Py_CLEAR(views[1]);
    observation_buffer_mode = 2;
    views[0] = PyMemoryView_FromObject(exporter);
    BUFFER_REQUIRE(views[0] != NULL && strlen(PyMemoryView_GET_BUFFER(views[0])->format) > 16, 433);
    BUFFER_REQUIRE(PyObject_GetBuffer(views[0], &guarded.view, PyBUF_FULL_RO) == 0
        && strcmp(guarded.view.format, PyMemoryView_GET_BUFFER(views[0])->format) == 0, 434);
    PyBuffer_Release(&guarded.view);
    Py_CLEAR(views[0]);
    observation_buffer_mode = 0;
    before = observation_buffer_releases;
    views[0] = PyMemoryView_FromObject(exporter);
    BUFFER_REQUIRE(views[0] != NULL && PyMemoryView_GET_BASE(views[0]) == exporter, 414);
    views[1] = PyMemoryView_FromObject(views[0]);
    BUFFER_REQUIRE(views[1] != NULL && PyMemoryView_GET_BASE(views[1]) == exporter, 415);
    Py_CLEAR(views[0]);
    BUFFER_REQUIRE(observation_buffer_releases == before
        && PyMemoryView_GET_BUFFER(views[1])->len == 3, 416);
    Py_CLEAR(views[1]);
    BUFFER_REQUIRE(observation_buffer_releases == before + 1
        && observation_buffer_acquires == observation_buffer_releases
        && Py_REFCNT(exporter) == refs && PyErr_Occurred() == NULL, 417);
done:
    {
        PyObject *error = PyErr_GetRaisedException();
        observation_release_error = observation_buffer_mode = 0;
        Py_XDECREF(result);
        for (int i = 0; i < 65; ++i) Py_XDECREF(views[i]);
        PyErr_SetRaisedException(error);
    }
#undef BUFFER_REQUIRE
    return status;
}

static int check_object_observation_authority(PyObject **types, BufferConstructorProbe constructor) {
    PyType_Slot observer_slots[] = {
        {Py_tp_new, (void *)PyType_GenericNew},
        {Py_tp_getattro, (void *)observation_getattr},
        {Py_tp_hash, (void *)observation_hash},
        {Py_tp_repr, (void *)observation_string},
        {Py_tp_str, (void *)observation_string},
        {Py_sq_length, (void *)observation_length},
        {Py_tp_methods, observation_methods},
        {Py_bf_getbuffer, (void *)observation_buffer},
        {Py_bf_releasebuffer, (void *)observation_releasebuffer},
        {Py_tp_richcompare, (void *)observation_compare},
        {Py_nb_bool, (void *)observation_bool}, {0, NULL}
    };
    PyType_Slot reflected_slots[] = {
        {Py_tp_richcompare, (void *)observation_reflected},
        {Py_tp_call, (void *)observation_call}, {0, NULL}
    };
    PyType_Slot meta_slots[] = {{Py_tp_methods, observation_meta_methods}, {0, NULL}};
    PyType_Slot empty[] = {{0, NULL}};
    PyType_Slot buffer_slots[] = {
        {Py_bf_getbuffer, (void *)observation_buffer},
        {Py_bf_releasebuffer, (void *)observation_releasebuffer},
        {Py_sq_item, (void *)observation_sequence_item},
        {Py_sq_length, (void *)observation_sequence_length},
        {Py_nb_index, (void *)observation_buffer_index}, {0, NULL}
    };
    PyType_Slot sequence_slots[] = {
        {Py_sq_item, (void *)observation_sequence_item},
        {Py_sq_length, (void *)observation_sequence_length},
        {Py_tp_getattro, (void *)observation_getattr}, {0, NULL}
    };
    PyType_Spec buffer_spec = {"factory.NativeBuffer", sizeof(PyObject), 0, 0, buffer_slots};
    PyType_Spec sequence_spec = {"factory.NativeSequence", sizeof(PyObject), 0, 0, sequence_slots};
    PyType_Spec observer = {"factory.Observer", sizeof(PyObject), 0, Py_TPFLAGS_BASETYPE, observer_slots};
    PyType_Spec reflected = {"factory.Reflected", 0, 0, Py_TPFLAGS_BASETYPE, reflected_slots};
    PyType_Spec meta = {"factory.ObservationMeta", 0, 0, Py_TPFLAGS_BASETYPE, meta_slots};
    PyType_Spec checked = {"factory.Checked", sizeof(PyObject), 0, 0, empty};
    PyObject *left = NULL, *right = NULL, *bases = NULL, *result = NULL, *classes = NULL;
    PyObject *spec = NULL, *values = NULL, *opaque = NULL;
    int (*exact_type)(PyObject *) = PyType_CheckExact;
    int status = 0;
    observation_attributes = observation_truth_calls = observation_left_calls = observation_right_calls = 0;
    observation_compare_mode = observation_check_calls = observation_check_fails = 0;
    observation_truth = 1;
    observation_conversion_mode = observation_length_calls = 0;
    observation_dir_result = NULL;
    observation_sequence_names = 0;
#define OBSERVE(condition, code) do { if (!(condition)) { status = (code); goto done; } } while (0)
    PyErr_SetString(PyExc_LookupError, "original observation failure");
    observation_failure = PyErr_GetRaisedException();
    OBSERVE(observation_failure != NULL, 212);
    types[7] = PyType_FromSpec(&observer);
    OBSERVE(types[7] != NULL, 213);
    bases = PyTuple_Pack(1, types[7]);
    OBSERVE(bases != NULL, 214);
    types[8] = PyType_FromSpecWithBases(&reflected, bases);
    Py_CLEAR(bases);
    OBSERVE(types[8] != NULL, 215);
    left = PyType_GenericNew((PyTypeObject *)types[7], NULL, NULL);
    right = PyType_GenericNew((PyTypeObject *)types[8], NULL, NULL);
    OBSERVE(left != NULL && right != NULL, 216);
    observation_result = left;
    status = check_private_observation_admission(left);
    if (status != 0) goto done;
    result = PyObject_Repr(left);
    OBSERVE(text_equals(result, "native-string"), 300);
    Py_CLEAR(result);
    result = PyObject_Str(left);
    OBSERVE(text_equals(result, "native-string"), 301);
    Py_CLEAR(result);
    OBSERVE(PyObject_Length(left) == 7 && PyObject_Size(left) == 7
        && observation_length_calls == 2, 302);
    observation_buffer_acquires = observation_buffer_releases = 0;
    result = PyObject_Bytes(left);
    OBSERVE(result != NULL && PyBytes_Size(result) == 12
        && memcmp(PyBytes_AsString(result), "native-bytes", 12) == 0
        && observation_buffer_acquires == 0, 303);
    Py_CLEAR(result);
    result = constructor(left, 0);
    OBSERVE(result != NULL && PyBytes_Size(result) == 12
        && memcmp(PyBytes_AsString(result), "native-bytes", 12) == 0
        && observation_buffer_acquires == 0, 418);
    Py_CLEAR(result);
    spec = PyUnicode_FromString(".native");
    OBSERVE(spec != NULL, 304);
    result = PyObject_Format(left, spec);
    OBSERVE(text_equals(result, "native-format"), 305);
    Py_CLEAR(result);
    result = PyObject_Format(left, NULL);
    OBSERVE(text_equals(result, "native-format"), 306);
    Py_CLEAR(result);
    result = PyObject_Dir(left);
    OBSERVE(result != NULL && PyList_Size(result) == 2
        && text_equals(PyList_GetItem(result, 0), "a-native")
        && text_equals(PyList_GetItem(result, 1), "z-native"), 307);
    Py_CLEAR(result);
    OBSERVE(observation_attributes == 0, 308);
    observation_conversion_mode = 1;
    OBSERVE(PyObject_Repr(left) == NULL && observation_error_is_original(), 309);
    OBSERVE(PyObject_Str(left) == NULL && observation_error_is_original(), 310);
    OBSERVE(PyObject_Length(left) == -1 && observation_error_is_original(), 311);
    OBSERVE(PyObject_Size(left) == -1 && observation_error_is_original(), 312);
    OBSERVE(PyObject_Bytes(left) == NULL && observation_error_is_original(), 313);
    OBSERVE(PyObject_Format(left, spec) == NULL && observation_error_is_original(), 314);
    OBSERVE(PyObject_Dir(left) == NULL && observation_error_is_original(), 315);
    observation_conversion_mode = 2;
    OBSERVE(PyObject_Repr(left) == NULL && PyErr_ExceptionMatches(PyExc_TypeError), 316);
    PyErr_Clear();
    OBSERVE(PyObject_Str(left) == NULL && PyErr_ExceptionMatches(PyExc_TypeError), 317);
    PyErr_Clear();
    OBSERVE(PyObject_Bytes(left) == NULL && PyErr_ExceptionMatches(PyExc_TypeError), 318);
    PyErr_Clear();
    OBSERVE(PyObject_Format(left, spec) == NULL && PyErr_ExceptionMatches(PyExc_TypeError), 319);
    PyErr_Clear();
    observation_conversion_mode = 0;
    OBSERVE(PyObject_Format(left, Py_None) == NULL && PyErr_ExceptionMatches(PyExc_SystemError), 320);
    PyErr_Clear();
    values = Py_BuildValue("[ii]", 65, 255);
    OBSERVE(values != NULL, 321);
    result = PyObject_Bytes(values);
    OBSERVE(result != NULL && PyBytes_Size(result) == 2
        && (unsigned char)PyBytes_AsString(result)[0] == 65
        && (unsigned char)PyBytes_AsString(result)[1] == 255, 322);
    Py_CLEAR(result);
    Py_CLEAR(values);
    values = PyLong_FromLong(3);
    OBSERVE(values != NULL, 323);
    OBSERVE(PyObject_Bytes(values) == NULL && PyErr_ExceptionMatches(PyExc_TypeError), 324);
    PyErr_Clear();
    opaque = PyType_GenericNew(&PyBaseObject_Type, NULL, NULL);
    OBSERVE(opaque != NULL, 325);
    OBSERVE(PyObject_Bytes(opaque) == NULL && PyErr_ExceptionMatches(PyExc_TypeError), 326);
    PyErr_Clear();
    result = PyObject_Bytes(NULL);
    OBSERVE(result != NULL && PyBytes_Size(result) == 6
        && memcmp(PyBytes_AsString(result), "<NULL>", 6) == 0, 327);
    Py_CLEAR(result);
    OBSERVE(PyObject_Type(NULL) == NULL && PyErr_ExceptionMatches(PyExc_SystemError), 328);
    PyErr_Clear();
    OBSERVE(PyObject_Length(NULL) == -1 && PyErr_ExceptionMatches(PyExc_SystemError), 329);
    PyErr_Clear();
    OBSERVE(PyObject_Size(NULL) == -1 && PyErr_ExceptionMatches(PyExc_SystemError), 330);
    PyErr_Clear();
    observation_raise();
    OBSERVE(PyObject_Type(NULL) == NULL && observation_error_is_original(), 331);
    observation_raise();
    OBSERVE(PyObject_Length(NULL) == -1 && observation_error_is_original(), 332);
    observation_raise();
    OBSERVE(PyObject_Size(NULL) == -1 && observation_error_is_original(), 333);
    Py_CLEAR(spec);
    Py_CLEAR(values);
    Py_CLEAR(opaque);
    types[11] = PyType_FromSpec(&buffer_spec);
    types[12] = PyType_FromSpec(&sequence_spec);
    OBSERVE(types[11] != NULL && types[12] != NULL, 334);
    opaque = PyType_GenericNew((PyTypeObject *)types[11], NULL, NULL);
    OBSERVE(opaque != NULL, 335);
    observation_buffer_acquires = observation_buffer_releases = 0;
    Py_ssize_t buffer_refs = Py_REFCNT(opaque);
    result = PyObject_Bytes(opaque);
    OBSERVE(result != NULL && PyBytes_Size(result) == 3
        && memcmp(PyBytes_AsString(result), "ABC", 3) == 0
        && observation_buffer_acquires == 1 && observation_buffer_releases == 1
        && Py_REFCNT(opaque) == buffer_refs, 336);
    Py_CLEAR(result);
    observation_conversion_mode = 1;
    OBSERVE(PyObject_Bytes(opaque) == NULL && observation_error_is_original()
        && observation_buffer_releases == 1 && Py_REFCNT(opaque) == buffer_refs, 337);
    OBSERVE(constructor(opaque, 0) == NULL && observation_error_is_original()
        && observation_buffer_releases == 1 && Py_REFCNT(opaque) == buffer_refs, 419);
    observation_conversion_mode = 0;
    status = check_buffer_authority(opaque, constructor);
    if (status != 0) goto done;
    Py_CLEAR(opaque);
    opaque = PyType_GenericNew((PyTypeObject *)types[12], NULL, NULL);
    OBSERVE(opaque != NULL, 338);
    result = PyObject_Bytes(opaque);
    OBSERVE(result != NULL && PyBytes_Size(result) == 2
        && (unsigned char)PyBytes_AsString(result)[0] == 65
        && (unsigned char)PyBytes_AsString(result)[1] == 255, 339);
    Py_CLEAR(result);
    observation_conversion_mode = 1;
    OBSERVE(PyObject_Bytes(opaque) == NULL && observation_error_is_original(), 340);
    observation_conversion_mode = 0;
    OBSERVE(PyObject_Dir(opaque) == NULL && observation_error_is_original(), 341);
    observation_sequence_names = 1;
    observation_dir_result = opaque;
    result = PyObject_Dir(left);
    observation_dir_result = NULL;
    OBSERVE(result != NULL && PyList_Size(result) == 2
        && text_equals(PyList_GetItem(result, 0), "a-native")
        && text_equals(PyList_GetItem(result, 1), "z-native"), 343);
    Py_CLEAR(result);
    observation_sequence_names = 0;
    Py_CLEAR(opaque);
    observation_attributes = 0;
    OBSERVE(PyType_Check(types[7]) && exact_type(types[7]) && PyType_CheckExact(types[7])
        && !PyType_Check(left), 217);
    OBSERVE(PyType_IsSubtype((PyTypeObject *)types[8], (PyTypeObject *)types[7]) == 1
        && PyType_IsSubtype((PyTypeObject *)types[7], (PyTypeObject *)types[8]) == 0, 218);
    OBSERVE(PyObject_TypeCheck(right, (PyTypeObject *)types[7]) == 1
        && PyObject_TypeCheck(left, &PyLong_Type) == 0 && observation_attributes == 0, 219);
    result = PyObject_Type(right);
    OBSERVE(result == types[8], 220);
    Py_CLEAR(result);
    OBSERVE(PyObject_IsSubclass(types[8], types[7]) == 1
        && PyObject_IsSubclass(types[7], types[8]) == 0, 221);
    OBSERVE(PyType_IsSubtype((PyTypeObject *)types[4], (PyTypeObject *)types[3]) == 1
        && PyObject_IsSubclass(types[4], types[3]) == 1, 222);
    OBSERVE(PyObject_IsInstance(right, types[7]) == 1 && observation_attributes == 0, 223);
    OBSERVE(PyObject_Hash(left) == 867 && observation_attributes == 0, 224);
    OBSERVE(PyCallable_Check(left) == 0 && PyCallable_Check(right) == 1
        && observation_attributes == 0, 225);
    result = PyObject_GetAttrString(left, "__call__");
    OBSERVE(result == Py_True, 226);
    Py_CLEAR(result);
    observation_attributes = 0;
    Py_ssize_t refs = Py_REFCNT(left);
    result = PyObject_RichCompare(left, right, Py_LT);
    OBSERVE(result == left && Py_REFCNT(left) == refs + 1 && observation_truth_calls == 0
        && observation_right_calls == 1 && observation_left_calls == 0
        && observation_opcode == Py_GT && observation_attributes == 0, 227);
    Py_CLEAR(result);
    OBSERVE(Py_REFCNT(left) == refs && PyObject_RichCompareBool(left, right, Py_LT) == 1
        && observation_truth_calls == 1, 228);
    observation_left_calls = observation_right_calls = 0;
    OBSERVE(PyObject_RichCompareBool(left, left, Py_EQ) == 1
        && PyObject_RichCompareBool(left, left, Py_NE) == 0
        && observation_left_calls == 0 && observation_truth_calls == 1, 229);
    result = PyObject_RichCompare(left, left, Py_EQ);
    OBSERVE(result == left && observation_left_calls == 1, 230);
    Py_CLEAR(result);
    observation_truth = -2;
    OBSERVE(PyObject_IsTrue(left) == -2 && observation_error_is_original(), 231);
    OBSERVE(PyObject_Not(left) == -2 && observation_error_is_original(), 232);
    OBSERVE(PyObject_RichCompareBool(left, right, Py_LT) == -2
        && observation_error_is_original() && Py_REFCNT(left) == refs, 233);
    observation_truth = 1;
    observation_compare_mode = 2;
    result = PyObject_RichCompare(left, right, Py_LT);
    OBSERVE(result == NULL && observation_error_is_original(), 234);
    observation_compare_mode = 1;
    result = PyObject_RichCompare(left, right, Py_EQ);
    OBSERVE(result == Py_False, 235);
    Py_CLEAR(result);
    result = PyObject_RichCompare(left, right, Py_LT);
    OBSERVE(result == NULL && PyErr_ExceptionMatches(PyExc_TypeError), 236);
    PyErr_Clear();
    observation_compare_mode = 0;
    OBSERVE(PyObject_HashNotImplemented(left) == -1 && PyErr_ExceptionMatches(PyExc_TypeError), 237);
    PyErr_Clear();
    OBSERVE(PyObject_IsInstance(left, (PyObject *)&PyLong_Type) == -1
        && observation_error_is_original(), 238);
    OBSERVE(PyObject_IsSubclass(left, types[7]) == -1 && observation_error_is_original(), 239);

    bases = PyTuple_Pack(1, (PyObject *)&PyType_Type);
    OBSERVE(bases != NULL, 240);
    types[9] = PyType_FromSpecWithBases(&meta, bases);
    Py_CLEAR(bases);
    OBSERVE(types[9] != NULL, 241);
    types[10] = PyType_FromMetaclass((PyTypeObject *)types[9], NULL, &checked, NULL);
    OBSERVE(types[10] != NULL && PyType_Check(types[10]) && !PyType_CheckExact(types[10]), 242);
    observation_check_calls = 0;
    int classinfo_truth_calls = observation_truth_calls;
    /* Separate failures without weakening the original conjunction. */
    OBSERVE(PyObject_IsInstance(left, types[10]) == 1, 243);
    OBSERVE(observation_check_calls == 1, 440);
    OBSERVE(observation_truth_calls == classinfo_truth_calls + 1, 441);
    OBSERVE(PyObject_IsSubclass(types[7], types[10]) == 1, 442);
    OBSERVE(observation_check_calls == 2, 443);
    OBSERVE(observation_truth_calls == classinfo_truth_calls + 2, 444);
    classes = PyTuple_Pack(2, types[10], Py_None);
    OBSERVE(classes != NULL, 244);
    OBSERVE(PyObject_IsInstance(left, classes) == 1 && PyObject_IsSubclass(types[7], classes) == 1
        && observation_check_calls == 4 && PyErr_Occurred() == NULL, 245);
    Py_CLEAR(classes);
    classes = PyTuple_Pack(2, Py_None, types[10]);
    OBSERVE(classes != NULL, 246);
    OBSERVE(PyObject_IsInstance(left, classes) == -1 && PyErr_ExceptionMatches(PyExc_TypeError)
        && observation_check_calls == 4, 247);
    PyErr_Clear();
    OBSERVE(PyObject_IsSubclass(types[7], classes) == -1 && PyErr_ExceptionMatches(PyExc_TypeError)
        && observation_check_calls == 4, 248);
    PyErr_Clear();
    observation_truth = 0;
    OBSERVE(PyObject_IsInstance(left, types[10]) == 0, 249);
    OBSERVE(PyObject_IsSubclass(types[7], types[10]) == 0, 445);
    OBSERVE(observation_check_calls == 6
        && observation_truth_calls == classinfo_truth_calls + 6, 446);
    /* The hook succeeds but its native result's truth slot raises. Both
     * class-info consumers must preserve that exact native exception. */
    observation_truth = -2;
    OBSERVE(PyObject_IsInstance(left, types[10]) == -1 && observation_error_is_original(), 447);
    OBSERVE(PyObject_IsSubclass(types[7], types[10]) == -1 && observation_error_is_original(), 448);
    OBSERVE(observation_check_calls == 8
        && observation_truth_calls == classinfo_truth_calls + 8, 449);
    observation_truth = 1;
    observation_check_fails = 1;
    OBSERVE(PyObject_IsInstance(left, types[10]) == -1 && observation_error_is_original(), 250);
    OBSERVE(PyObject_IsSubclass(types[7], types[10]) == -1 && observation_error_is_original(), 251);
    OBSERVE(PyErr_Occurred() == NULL, 252);
done:
    {
        PyObject *error = PyErr_GetRaisedException();
        observation_result = NULL;
        observation_dir_result = NULL;
        Py_CLEAR(observation_failure);
        Py_XDECREF(result);
        Py_XDECREF(classes);
        Py_XDECREF(bases);
        Py_XDECREF(spec);
        Py_XDECREF(values);
        Py_XDECREF(opaque);
        Py_XDECREF(right);
        Py_XDECREF(left);
        PyErr_SetRaisedException(error);
    }
#undef OBSERVE
    return status;
}

/* Rust owns the returned type references and clears their real type cycles
 * after inspecting physical layout. All APIs under test execute in C here. */
int MOLT_TYPE_FACTORY_PROBE(PyTypeObject *metaclass,
                            PyObject *module_a, PyObject *module_b,
                            PyModuleDef *def_a, PyModuleDef *def_b,
                            PyModuleDef *missing, PyObject **types, size_t *buffer_layout,
                            BufferConstructorProbe constructor) {
    PyType_Slot slots[] = {
        {Py_tp_new, (void *)PyType_GenericNew},
        {Py_tp_traverse, (void *)payload_traverse},
        {Py_tp_clear, (void *)payload_clear},
        {Py_tp_finalize, (void *)payload_finalize},
        {Py_tp_free, (void *)payload_free},
        {Py_tp_repr, (void *)payload_repr},
        {Py_tp_hash, (void *)payload_hash},
        {Py_tp_methods, payload_methods},
        {Py_tp_members, payload_members},
        {Py_tp_getset, payload_getsets},
        {0, NULL}
    };
    PyType_Slot empty[] = {{0, NULL}};
    PyType_Spec payload = {"factory.Payload", sizeof(FactoryPayload), 0,
        Py_TPFLAGS_BASETYPE | Py_TPFLAGS_HAVE_GC, slots};
    PyType_Spec derived = {"factory.Derived", 0, 0, Py_TPFLAGS_BASETYPE, empty};
    PyType_Spec left = {"factory.Left", sizeof(PyObject), 0, Py_TPFLAGS_BASETYPE, empty};
    PyType_Spec right = {"factory.Right", sizeof(PyObject), 0, Py_TPFLAGS_BASETYPE, empty};
    PyType_Spec selected = {"factory.Selected", sizeof(PyObject), 0, Py_TPFLAGS_BASETYPE, empty};
    PyType_Spec variable = {"factory.Variable", sizeof(PyVarObject), sizeof(Py_ssize_t), 0, empty};
    PyObject *bases = NULL, *instance = NULL, *number = NULL;
    PyObject *observed = NULL, *method = NULL, *var = NULL;
    PyObject *dict, *module;
    Py_ssize_t type_refs, module_a_refs, module_b_refs, dict_refs;
    unsigned long (*get_flags)(PyTypeObject *) = PyType_GetFlags;
    int status = 0;
    finalizations = frees = 0;
    /* Target data model facts cross back to the Rust repr(C) authority. */
    struct BufferAlignment { char prefix; Py_buffer value; };
    buffer_layout[0] = sizeof(Py_buffer);
    buffer_layout[1] = offsetof(struct BufferAlignment, value);
    buffer_layout[2] = offsetof(Py_buffer, buf);
    buffer_layout[3] = offsetof(Py_buffer, obj);
    buffer_layout[4] = offsetof(Py_buffer, len);
    buffer_layout[5] = offsetof(Py_buffer, itemsize);
    buffer_layout[6] = offsetof(Py_buffer, readonly);
    buffer_layout[7] = offsetof(Py_buffer, ndim);
    buffer_layout[8] = offsetof(Py_buffer, format);
    buffer_layout[9] = offsetof(Py_buffer, shape);
    buffer_layout[10] = offsetof(Py_buffer, strides);
    buffer_layout[11] = offsetof(Py_buffer, suboffsets);
    buffer_layout[12] = offsetof(Py_buffer, internal);
    for (int index = 0; index < 13; ++index) types[index] = NULL;

#define REQUIRE(condition, code) do { if (!(condition)) { status = (code); goto done; } } while (0)
    REQUIRE(PyType_Ready(metaclass) == 0, 1);
    types[0] = PyType_FromSpec(&payload);
    REQUIRE(types[0] != NULL, 2);
    REQUIRE((get_flags((PyTypeObject *)types[0])
        & (Py_TPFLAGS_HEAPTYPE | Py_TPFLAGS_READY | Py_TPFLAGS_HAVE_GC))
        == (Py_TPFLAGS_HEAPTYPE | Py_TPFLAGS_READY | Py_TPFLAGS_HAVE_GC), 3);
    REQUIRE(PyType_HasFeature((PyTypeObject *)types[0], Py_TPFLAGS_BASETYPE), 4);
    REQUIRE(PyType_IS_GC((PyTypeObject *)types[0]), 5);
    REQUIRE(PyType_GetSlot((PyTypeObject *)types[0], Py_tp_repr) == (void *)payload_repr, 6);
    REQUIRE(PyType_GetSlot((PyTypeObject *)types[0], Py_tp_traverse) == (void *)payload_traverse, 7);
    REQUIRE(PyType_GetSlot((PyTypeObject *)types[0], Py_tp_clear) == (void *)payload_clear, 8);
    dict = PyType_GetDict((PyTypeObject *)types[0]);
    REQUIRE(dict != NULL && PyDict_Check(dict), 9);
    dict_refs = Py_REFCNT(dict);
    observed = PyType_GetDict((PyTypeObject *)types[0]);
    REQUIRE(observed == dict && Py_REFCNT(dict) == dict_refs + 1, 10);
    Py_CLEAR(observed);
    REQUIRE(Py_REFCNT(dict) == dict_refs, 343);
    Py_CLEAR(dict);
    observed = PyType_GetName((PyTypeObject *)types[0]);
    REQUIRE(text_equals(observed, "Payload"), 11);
    Py_CLEAR(observed);
    observed = PyType_GetQualName((PyTypeObject *)types[0]);
    REQUIRE(text_equals(observed, "Payload"), 12);
    Py_CLEAR(observed);

    type_refs = Py_REFCNT(types[0]);
    instance = PyType_GenericNew((PyTypeObject *)types[0], NULL, NULL);
    REQUIRE(instance != NULL && Py_TYPE(instance) == (PyTypeObject *)types[0], 20);
    REQUIRE(Py_REFCNT(types[0]) == type_refs + 1 && PyObject_GC_IsTracked(instance), 21);
    REQUIRE(((FactoryPayload *)instance)->value == 0 && ((FactoryPayload *)instance)->edge == NULL, 22);
    ((FactoryPayload *)instance)->edge = PyLong_FromLong(731);
    REQUIRE(((FactoryPayload *)instance)->edge != NULL, 23);
    {
        VisitWitness witness = {((FactoryPayload *)instance)->edge, types[0], 0, 0};
        traverseproc traverse = (traverseproc)PyType_GetSlot((PyTypeObject *)types[0], Py_tp_traverse);
        REQUIRE(traverse(instance, visit_payload, &witness) == 0
            && witness.edge_visits == 1 && witness.type_visits == 1, 24);
    }
    number = PyLong_FromLong(41);
    REQUIRE(number != NULL && PyObject_SetAttrString(instance, "value", number) == 0, 25);
    REQUIRE(((FactoryPayload *)instance)->value == 41, 26);
    observed = PyObject_GetAttrString(instance, "current_value");
    REQUIRE(observed != NULL && PyLong_AsLong(observed) == 41, 27);
    Py_CLEAR(observed);
    method = PyObject_GetAttrString(instance, "next_value");
    REQUIRE(method != NULL, 28);
    observed = PyObject_CallObject(method, NULL);
    REQUIRE(observed != NULL && PyLong_AsLong(observed) == 42, 29);
    Py_CLEAR(observed);
    Py_CLEAR(method);
    observed = PyObject_Repr(instance);
    REQUIRE(text_equals(observed, "factory-native"), 30);
    Py_CLEAR(observed);
    observed = PyObject_Str(instance);
    REQUIRE(text_equals(observed, "factory-native"), 299);
    Py_CLEAR(observed);
    observed = PyObject_Dir(instance);
    REQUIRE(observed != NULL && observation_list_contains(observed, "next_value")
        && observation_list_contains(observed, "value"), 342);
    Py_CLEAR(observed);
    status = check_container_call_authority(instance, types[0], module_a, &types[6]);
    if (status != 0) goto done;
    REQUIRE(((int (*)(PyObject *))PyType_GetSlot((PyTypeObject *)types[0], Py_tp_clear))(instance) == 0
        && ((FactoryPayload *)instance)->edge == NULL, 31);
    Py_CLEAR(instance);
    REQUIRE(finalizations == 1 && frees == 1 && Py_REFCNT(types[0]) == type_refs, 32);

    bases = PyTuple_Pack(1, types[0]);
    REQUIRE(bases != NULL && PyErr_Occurred() == NULL
        && PyTuple_GetItem(bases, 0) == types[0], 40);
    types[1] = PyType_FromSpecWithBases(&derived, bases);
    Py_CLEAR(bases);
    REQUIRE(types[1] != NULL && PyType_GetSlot((PyTypeObject *)types[1], Py_tp_base) == types[0], 41);
    REQUIRE(PyType_GetSlot((PyTypeObject *)types[1], Py_tp_repr) == (void *)payload_repr
        && PyType_IS_GC((PyTypeObject *)types[1]), 42);
    types[2] = PyType_FromModuleAndSpec(module_a, &left, NULL);
    types[3] = PyType_FromModuleAndSpec(module_b, &right, NULL);
    REQUIRE(types[2] != NULL && types[3] != NULL, 43);
    bases = PyTuple_Pack(2, types[2], types[3]);
    REQUIRE(bases != NULL, 44);
    types[4] = PyType_FromMetaclass(metaclass, module_a, &selected, bases);
    Py_CLEAR(bases);
    REQUIRE(types[4] != NULL && Py_TYPE(types[4]) == metaclass, 45);
    REQUIRE(PyType_GetSlot((PyTypeObject *)types[4], Py_tp_base) == types[2], 46);

    module_a_refs = Py_REFCNT(module_a);
    module_b_refs = Py_REFCNT(module_b);
    for (int repeat = 0; repeat < 3; ++repeat) {
        REQUIRE(PyType_GetModule((PyTypeObject *)types[4]) == module_a, 50);
        REQUIRE(PyType_GetModuleState((PyTypeObject *)types[4]) == PyModule_GetState(module_a), 51);
        REQUIRE(PyType_GetModuleByDef((PyTypeObject *)types[4], def_a) == module_a, 52);
        /* Right is outside Selected's primary tp_base chain. Both module
         * definitions have slots, so PyState_FindModule cannot select one. */
        module = PyType_GetModuleByDef((PyTypeObject *)types[4], def_b);
        REQUIRE(module == module_b && PyModule_GetDef(module) == def_b, 53);
    }
    REQUIRE(Py_REFCNT(module_a) == module_a_refs && Py_REFCNT(module_b) == module_b_refs, 54);
    REQUIRE(PyType_GetModuleByDef((PyTypeObject *)types[4], missing) == NULL
        && PyErr_ExceptionMatches(PyExc_TypeError), 55);
    PyErr_Clear();
    REQUIRE(PyType_GetModule((PyTypeObject *)types[0]) == NULL
        && PyErr_ExceptionMatches(PyExc_TypeError), 56);
    PyErr_Clear();
    REQUIRE(PyType_GetModule(metaclass) == NULL && PyErr_ExceptionMatches(PyExc_TypeError), 57);
    PyErr_Clear();
    REQUIRE(PyType_GetSlot((PyTypeObject *)types[0], 0) == NULL
        && PyErr_ExceptionMatches(PyExc_SystemError), 58);
    PyErr_Clear();

    types[5] = PyType_FromSpec(&variable);
    REQUIRE(types[5] != NULL, 60);
    var = PyType_GenericAlloc((PyTypeObject *)types[5], 3);
    REQUIRE(var != NULL && Py_SIZE(var) == 3, 61);
    for (int index = 0; index < 3; ++index) {
        Py_ssize_t *items = (Py_ssize_t *)((PyVarObject *)var + 1);
        REQUIRE(items[index] == 0, 62);
        items[index] = (Py_ssize_t)(index + 17);
    }
    REQUIRE(PyErr_Occurred() == NULL, 63);
    status = check_object_observation_authority(types, constructor);
    if (status != 0) goto done;
done:
    Py_XDECREF(var);
    Py_XDECREF(observed);
    Py_XDECREF(method);
    Py_XDECREF(number);
    Py_XDECREF(instance);
    Py_XDECREF(bases);
#undef REQUIRE
    return status;
}

/* Managed dictionary storage and mapping materialization are called through
 * both shipped C header surfaces. Every failure drains temporary owners. */
#ifdef MOLT_PUBLIC_HEADER_PROBE
#define MAPPING_RESULT_PROBE molt_public_mapping_result_probe
#define DICT_SUBCLASS_PROBE molt_public_dict_subclass_probe
#else
#define MAPPING_RESULT_PROBE molt_linked_mapping_result_probe
#define DICT_SUBCLASS_PROBE molt_linked_dict_subclass_probe
#endif
int MAPPING_RESULT_PROBE(PyObject *mapping, int method, PyObject *expected, int identity) {
    PyObject *result = method == 0 ? PyMapping_Keys(mapping)
        : method == 1 ? PyMapping_Values(mapping) : PyMapping_Items(mapping);
    PyObject *error_type = NULL, *error_value = NULL, *error_tb = NULL;
    int status = 0;
    if (result == NULL) { status = 1; goto done; }
    if (!PyList_CheckExact(result)) { status = 2; goto done; }
    if (identity ? result != expected : PyObject_RichCompareBool(result, expected, Py_EQ) != 1) {
        status = 3; goto done;
    }
done:
    PyErr_Fetch(&error_type, &error_value, &error_tb);
    Py_XDECREF(result);
    PyErr_Restore(error_type, error_value, error_tb);
    return status;
}
int DICT_SUBCLASS_PROBE(PyObject *dict, PyObject *first, PyObject *second) {
    PyObject *target = NULL, *keys = NULL, *key = NULL, *value = NULL;
    PyObject *error_type = NULL, *error_value = NULL, *error_tb = NULL;
    Py_ssize_t position = 0;
    int status = 0;
#define DICT_REQUIRE(condition, code) do { if (!(condition)) { status = code; goto done; } } while (0)
    DICT_REQUIRE(PyDict_Check(dict), 1);
    DICT_REQUIRE(PyDict_SetItemString(dict, "a", first) == 0, 2);
    DICT_REQUIRE(PyDict_Size(dict) == 1, 3);
    DICT_REQUIRE(PyDict_GetItemString(dict, "a") == first && !PyErr_Occurred(), 4);
    DICT_REQUIRE(PyDict_Next(dict, &position, &key, &value) == 1 && value == first, 5);
    DICT_REQUIRE(PyDict_Next(dict, &position, &key, &value) == 0, 6);
    DICT_REQUIRE(PyDict_SetItemString(dict, "b", second) == 0 && PyDict_Size(dict) == 2, 7);
    DICT_REQUIRE(PyDict_GetItemString(dict, "b") == second, 8);
    target = PyDict_New();
    DICT_REQUIRE(target != NULL && PyDict_Update(target, dict) == 0, 9);
    DICT_REQUIRE(PyDict_Size(target) == 2 && PyDict_GetItemString(target, "b") == second, 10);
    keys = PyDict_Keys(dict);
    DICT_REQUIRE(keys != NULL && PyList_Size(keys) == 2, 11);
done:
    PyErr_Fetch(&error_type, &error_value, &error_tb);
    Py_XDECREF(keys);
    Py_XDECREF(target);
    PyErr_Restore(error_type, error_value, error_tb);
    return status;
#undef DICT_REQUIRE
}

#ifdef MOLT_PUBLIC_HEADER_PROBE
#define KNOWN_HASH_PROBE molt_public_known_hash_probe
#else
#define KNOWN_HASH_PROBE molt_linked_known_hash_probe
#endif
/* Borrowed identity, unchanged ownership and exact error identity are observed
 * by an actual consumer of the private declaration in each shipped header. */
int KNOWN_HASH_PROBE(PyObject *dict, PyObject *key, Py_hash_t hash,
                     PyObject *expected, PyObject *expected_error) {
    Py_ssize_t references = expected == NULL ? 0 : Py_REFCNT(expected);
    PyObject *result = _PyDict_GetItem_KnownHash(dict, key, hash);
    PyObject *error = PyErr_GetRaisedException();
    int status = result != expected ? 1 : error != expected_error ? 2
        : expected != NULL && Py_REFCNT(expected) != references ? 3 : 0;
    PyErr_SetRaisedException(error);
    return status;
}
