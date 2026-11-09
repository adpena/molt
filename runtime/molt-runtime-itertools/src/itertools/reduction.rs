//! CPython iterator reconstruction recipes and state validation.

use super::*;

fn iterator_recipe(py: &PyToken, class: u64, arguments: &[u64], state: Option<u64>) -> u64 {
    let Some(arguments) = reduction_tuple(py, arguments) else {
        return MoltObject::none().bits();
    };
    let mut recipe = vec![class, arguments.bits()];
    if let Some(state) = state {
        recipe.push(state);
    }
    reduction_tuple(py, &recipe).map_or(MoltObject::none().bits(), OwnedRuntimeValue::into_bits)
}

fn index_tuple<'py>(py: &'py PyToken, indices: &[usize]) -> Option<OwnedRuntimeValue<'py>> {
    let items: Vec<_> = indices
        .iter()
        .map(|&index| MoltObject::from_int(index as i64).bits())
        .collect();
    reduction_tuple(py, &items)
}

fn empty_list_iterator<'py>(py: &'py PyToken) -> Option<OwnedRuntimeValue<'py>> {
    let list = alloc_list(py, &[]);
    if list.is_null() {
        return None;
    }
    let list = unsafe { OwnedRuntimeValue::from_owned_bits(py, MoltObject::from_ptr(list).bits()) };
    let iterator = molt_iter(py, list.bits());
    if exception_pending(py) {
        return None;
    }
    Some(unsafe { OwnedRuntimeValue::from_owned_bits(py, iterator) })
}

pub fn itertools_reduce(py: &PyToken, shape: ObjectShapeId, instance: u64) -> u64 {
    let pointer = obj_from_bits(instance).as_ptr().unwrap();
    let class = unsafe { bridge::object_class_bits(pointer) };
    let none = MoltObject::none().bits();
    let integer = |value| MoltObject::from_int(value).bits();
    unsafe {
        match shape {
            ObjectShapeId::ItertoolsChain => {
                let source = chain_iterables_bits(pointer);
                if source == 0 || obj_from_bits(source).is_none() {
                    return iterator_recipe(py, class, &[], None);
                }
                let current = chain_current_bits(pointer);
                let values = if current == 0 || obj_from_bits(current).is_none() {
                    vec![source]
                } else {
                    vec![source, current]
                };
                let Some(state) = reduction_tuple(py, &values) else {
                    return none;
                };
                iterator_recipe(py, class, &[], Some(state.bits()))
            }
            ObjectShapeId::ItertoolsIslice => {
                let source = islice_iter_bits(pointer);
                if source == 0 || obj_from_bits(source).is_none() {
                    let Some(source) = empty_list_iterator(py) else {
                        return none;
                    };
                    return iterator_recipe(
                        py,
                        class,
                        &[source.bits(), integer(0)],
                        Some(integer(0)),
                    );
                }
                iterator_recipe(
                    py,
                    class,
                    &[
                        source,
                        integer(islice_next_idx(pointer)),
                        if islice_has_stop(pointer) == 0 {
                            none
                        } else {
                            integer(islice_stop(pointer))
                        },
                        integer(islice_step(pointer)),
                    ],
                    Some(integer(islice_idx(pointer))),
                )
            }
            ObjectShapeId::ItertoolsRepeat => {
                let times = repeat_times(pointer);
                let mut arguments = vec![repeat_obj_bits(pointer)];
                if times >= 0 {
                    arguments.push(integer(times));
                }
                iterator_recipe(py, class, &arguments, None)
            }
            ObjectShapeId::ItertoolsCount => {
                let current = count_current_bits(pointer);
                let step = count_step_bits(pointer);
                // CPython's compact counter omits its unit step. The sentinel
                // maximum switches to the arbitrary-precision representation.
                let compact = obj_from_bits(current)
                    .as_int()
                    .is_some_and(|value| value < i64::MAX)
                    && obj_from_bits(step).as_int() == Some(1);
                let arguments = [current, step];
                iterator_recipe(py, class, &arguments[..if compact { 1 } else { 2 }], None)
            }
            ObjectShapeId::ItertoolsCycle => {
                let Some((source, saved)) = cycle_payload(py, pointer) else {
                    return none;
                };
                let phase = cycle_phase(pointer);
                let source = if obj_from_bits(source.bits()).is_none() {
                    let iterator = molt_iter(py, saved.bits());
                    if exception_pending(py) {
                        return none;
                    }
                    let iterator = OwnedRuntimeValue::from_owned_bits(py, iterator);
                    let CyclePhase::Replaying(index) = phase else {
                        unreachable!("an exhausted cycle is replaying");
                    };
                    for _ in 0..index {
                        match iter_next_owned(py, &iterator) {
                            Ok(Some(_)) => (),
                            _ => return none,
                        }
                    }
                    iterator
                } else {
                    source
                };
                let Some(state) = reduction_tuple(
                    py,
                    &[
                        saved.bits(),
                        MoltObject::from_bool(phase != CyclePhase::Saving).bits(),
                    ],
                ) else {
                    return none;
                };
                iterator_recipe(py, class, &[source.bits()], Some(state.bits()))
            }
            ObjectShapeId::ItertoolsAccumulate => {
                let source = accumulate_iter_bits(pointer);
                let function = accumulate_func_bits(pointer);
                let function = if function == 0 { none } else { function };
                match accumulate_phase(pointer) {
                    AccumulatePhase::InitialPending => {
                        let Some(initial) =
                            reduction_tuple(py, &[accumulate_initial_bits(pointer)])
                        else {
                            return none;
                        };
                        let Some(iterables) = reduction_tuple(py, &[initial.bits(), source]) else {
                            return none;
                        };
                        let chain = molt_itertools_chain_new_attached_body(
                            py,
                            chain_class(py),
                            iterables.bits(),
                        );
                        if exception_pending(py) {
                            return none;
                        }
                        let chain = OwnedRuntimeValue::from_owned_bits(py, chain);
                        iterator_recipe(py, class, &[chain.bits(), function], Some(none))
                    }
                    AccumulatePhase::TotalReady
                        if obj_from_bits(accumulate_total_bits(pointer)).is_none() =>
                    {
                        let Some(initial) = reduction_tuple(py, &[none]) else {
                            return none;
                        };
                        let Some(iterables) = reduction_tuple(py, &[initial.bits(), source]) else {
                            return none;
                        };
                        let chain = molt_itertools_chain_new_attached_body(
                            py,
                            chain_class(py),
                            iterables.bits(),
                        );
                        if exception_pending(py) {
                            return none;
                        }
                        let chain = OwnedRuntimeValue::from_owned_bits(py, chain);
                        let accumulated = molt_itertools_accumulate_new_attached_body(
                            py,
                            class,
                            chain.bits(),
                            function,
                            none,
                        );
                        if exception_pending(py) {
                            return none;
                        }
                        let accumulated = OwnedRuntimeValue::from_owned_bits(py, accumulated);
                        iterator_recipe(
                            py,
                            islice_class(py),
                            &[accumulated.bits(), integer(1), none],
                            None,
                        )
                    }
                    phase => iterator_recipe(
                        py,
                        class,
                        &[source, function],
                        Some(if phase == AccumulatePhase::AwaitingFirst {
                            none
                        } else {
                            accumulate_total_bits(pointer)
                        }),
                    ),
                }
            }
            ObjectShapeId::ItertoolsCompress => iterator_recipe(
                py,
                class,
                &[
                    compress_data_iter_bits(pointer),
                    compress_selectors_iter_bits(pointer),
                ],
                None,
            ),
            ObjectShapeId::ItertoolsDropwhile => iterator_recipe(
                py,
                class,
                &[
                    dropwhile_predicate_bits(pointer),
                    dropwhile_iter_bits(pointer),
                ],
                Some(integer(i64::from(dropwhile_dropping(pointer) == 0))),
            ),
            ObjectShapeId::ItertoolsTakewhile => iterator_recipe(
                py,
                class,
                &[
                    takewhile_predicate_bits(pointer),
                    takewhile_iter_bits(pointer),
                ],
                Some(integer(takewhile_done(pointer))),
            ),
            ObjectShapeId::ItertoolsFilterfalse => iterator_recipe(
                py,
                class,
                &[
                    filterfalse_predicate_bits(pointer),
                    filterfalse_iter_bits(pointer),
                ],
                None,
            ),
            ObjectShapeId::ItertoolsStarmap => iterator_recipe(
                py,
                class,
                &[starmap_func_bits(pointer), starmap_iter_bits(pointer)],
                None,
            ),
            ObjectShapeId::ItertoolsGroupby => {
                let arguments = [groupby_iter_bits(pointer), groupby_keyfunc_bits(pointer)];
                if groupby_state(pointer) & (GROUPBY_HAS_TARGET | GROUPBY_HAS_CURRENT)
                    == (GROUPBY_HAS_TARGET | GROUPBY_HAS_CURRENT)
                {
                    let Some(state) = reduction_tuple(
                        py,
                        &[
                            groupby_curr_key_bits(pointer),
                            groupby_curr_val_bits(pointer),
                            groupby_tgt_key_bits(pointer),
                        ],
                    ) else {
                        return none;
                    };
                    iterator_recipe(py, class, &arguments, Some(state.bits()))
                } else {
                    iterator_recipe(py, class, &arguments, None)
                }
            }
            ObjectShapeId::ItertoolsProduct => {
                let data = &*product_data_ptr(pointer);
                if data.done {
                    let Some(empty) = reduction_tuple(py, &[]) else {
                        return none;
                    };
                    return iterator_recipe(py, class, &[empty.bits()], None);
                }
                let state = if data.first {
                    None
                } else {
                    index_tuple(py, &data.indices)
                };
                if exception_pending(py) {
                    return none;
                }
                iterator_recipe(
                    py,
                    class,
                    &data.pools_bits,
                    state.as_ref().map(OwnedRuntimeValue::bits),
                )
            }
            ObjectShapeId::ItertoolsCombinations
            | ObjectShapeId::ItertoolsCombinationsWithReplacement => {
                let (pool, r, first, done, indices) =
                    if shape == ObjectShapeId::ItertoolsCombinations {
                        let data = &*combinations_data_ptr(pointer);
                        (data.pool_bits, data.r, data.first, data.done, &data.indices)
                    } else {
                        let data = &*combinations_with_replacement_data_ptr(pointer);
                        (data.pool_bits, data.r, data.first, data.done, &data.indices)
                    };
                let empty = if !first && done {
                    reduction_tuple(py, &[])
                } else {
                    None
                };
                let state = if !first && !done {
                    index_tuple(py, indices)
                } else {
                    None
                };
                if exception_pending(py) {
                    return none;
                }
                iterator_recipe(
                    py,
                    class,
                    &[
                        empty.as_ref().map_or(pool, OwnedRuntimeValue::bits),
                        integer(r as i64),
                    ],
                    state.as_ref().map(OwnedRuntimeValue::bits),
                )
            }
            ObjectShapeId::ItertoolsPermutations => {
                let data = &*permutations_data_ptr(pointer);
                let empty = if !data.first && data.done {
                    reduction_tuple(py, &[])
                } else {
                    None
                };
                let state = if !data.first && !data.done {
                    let Some(indices) = index_tuple(py, &data.indices) else {
                        return none;
                    };
                    let Some(cycles) = index_tuple(py, &data.cycles) else {
                        return none;
                    };
                    reduction_tuple(py, &[indices.bits(), cycles.bits()])
                } else {
                    None
                };
                if exception_pending(py) {
                    return none;
                }
                iterator_recipe(
                    py,
                    class,
                    &[
                        empty
                            .as_ref()
                            .map_or(data.pool_bits, OwnedRuntimeValue::bits),
                        integer(data.r as i64),
                    ],
                    state.as_ref().map(OwnedRuntimeValue::bits),
                )
            }
            ObjectShapeId::ItertoolsZipLongest => {
                let data = &*zip_longest_data_ptr(pointer);
                let Some(empty) = reduction_tuple(py, &[]) else {
                    return none;
                };
                let arguments: Vec<_> = data
                    .iter_bits
                    .iter()
                    .map(|&bits| {
                        if bits == 0 || obj_from_bits(bits).is_none() {
                            empty.bits()
                        } else {
                            bits
                        }
                    })
                    .collect();
                iterator_recipe(py, class, &arguments, Some(data.fillvalue_bits))
            }
            _ => raise_exception::<u64>(
                py,
                BuiltinExc::TypeError,
                "cannot pickle this itertools iterator",
            ),
        }
    }
}

fn state_items(
    py: &PyToken,
    state: u64,
    length: Option<usize>,
    value_error: bool,
) -> Option<molt_runtime_core::OwnedBridgeHandleSnapshot<'_>> {
    let kind = if value_error {
        BuiltinExc::ValueError
    } else {
        BuiltinExc::TypeError
    };
    let Some(pointer) = obj_from_bits(state)
        .as_ptr()
        .filter(|&p| unsafe { object_type_id(p) } == TYPE_ID_TUPLE)
    else {
        raise_exception::<u64>(
            py,
            kind,
            if value_error {
                "invalid arguments"
            } else {
                "state is not a tuple"
            },
        );
        return None;
    };
    let items = unsafe { seq_snapshot(py, pointer) };
    if length.is_some_and(|length| length != items.len()) {
        raise_exception::<u64>(py, kind, "invalid arguments");
        return None;
    }
    Some(items)
}

fn state_indices(py: &PyToken, state: u64, length: usize) -> Option<Vec<i64>> {
    let items = state_items(py, state, Some(length), true)?;
    let mut indices = Vec::with_capacity(length);
    for &item in items.iter() {
        let index = index_i64_from_obj(py, item, "an integer is required");
        if exception_pending(py) {
            return None;
        }
        indices.push(index);
    }
    Some(indices)
}

unsafe fn replace_iterator_edge(
    py: &PyToken,
    pointer: *mut u8,
    value: u64,
    read: unsafe fn(*mut u8) -> u64,
    write: unsafe fn(*mut u8, u64),
) {
    inc_ref_bits(py, value);
    let old = unsafe { read(pointer) };
    unsafe {
        write(pointer, value);
    }
    dec_ref_bits(py, old);
}

pub fn itertools_setstate(py: &PyToken, shape: ObjectShapeId, instance: u64, state: u64) -> u64 {
    let pointer = obj_from_bits(instance).as_ptr().unwrap();
    let none = MoltObject::none().bits();
    unsafe {
        match shape {
            ObjectShapeId::ItertoolsChain => {
                let Some(items) = state_items(py, state, None, false) else {
                    return none;
                };
                if !(1..=2).contains(&items.len()) {
                    return raise_exception::<u64>(
                        py,
                        BuiltinExc::TypeError,
                        "invalid chain state",
                    );
                }
                let source = OwnedRuntimeValue::retain(py, items[0]);
                let active = OwnedRuntimeValue::retain(py, *items.get(1).unwrap_or(&none));
                let old = [chain_iterables_bits(pointer), chain_current_bits(pointer)];
                chain_set_iterables_bits(pointer, source.into_bits());
                chain_set_current_bits(pointer, active.into_bits());
                for value in old {
                    dec_ref_bits(py, value);
                }
            }
            ObjectShapeId::ItertoolsIslice => {
                let index = index_i64_from_obj(py, state, "an integer is required");
                if exception_pending(py) {
                    return none;
                }
                islice_set_idx(pointer, index);
            }
            ObjectShapeId::ItertoolsCycle => {
                let Some(items) = state_items(py, state, Some(2), false) else {
                    return none;
                };
                if !obj_from_bits(items[0])
                    .as_ptr()
                    .is_some_and(|p| object_type_id(p) == TYPE_ID_LIST)
                {
                    return raise_exception::<u64>(
                        py,
                        BuiltinExc::TypeError,
                        "argument 1 must be list",
                    );
                }
                let firstpass = index_i64_from_obj(py, items[1], "an integer is required");
                if exception_pending(py) {
                    return none;
                }
                let Some((source, _saved)) = cycle_payload(py, pointer) else {
                    return none;
                };
                let Some(payload) = reduction_tuple(py, &[source.bits(), items[0]]) else {
                    return none;
                };
                let old = cycle_saved_bits(pointer);
                cycle_set_saved_bits(pointer, payload.into_bits());
                cycle_set_phase(
                    pointer,
                    if obj_from_bits(source.bits()).is_none() {
                        CyclePhase::Replaying(0)
                    } else if firstpass != 0 {
                        CyclePhase::RestoredSource
                    } else {
                        CyclePhase::Saving
                    },
                );
                dec_ref_bits(py, old);
            }
            ObjectShapeId::ItertoolsAccumulate => {
                replace_iterator_edge(
                    py,
                    pointer,
                    state,
                    accumulate_total_bits,
                    accumulate_set_total_bits,
                );
                accumulate_set_phase(pointer, AccumulatePhase::TotalReady);
            }
            ObjectShapeId::ItertoolsDropwhile | ObjectShapeId::ItertoolsTakewhile => {
                let flag = is_truthy(py, obj_from_bits(state));
                if exception_pending(py) {
                    return none;
                }
                if shape == ObjectShapeId::ItertoolsDropwhile {
                    dropwhile_set_dropping(pointer, i64::from(!flag));
                } else {
                    takewhile_set_done(pointer, i64::from(flag));
                }
            }
            ObjectShapeId::ItertoolsGroupby => {
                let Some(items) = state_items(py, state, Some(3), false) else {
                    return none;
                };
                let values = items
                    .iter()
                    .map(|&bits| OwnedRuntimeValue::retain(py, bits))
                    .collect::<Vec<_>>();
                let old = [
                    groupby_curr_key_bits(pointer),
                    groupby_curr_val_bits(pointer),
                    groupby_tgt_key_bits(pointer),
                ];
                let mut values = values.into_iter();
                groupby_set_curr_key_bits(pointer, values.next().unwrap().into_bits());
                groupby_set_curr_val_bits(pointer, values.next().unwrap().into_bits());
                groupby_set_tgt_key_bits(pointer, values.next().unwrap().into_bits());
                groupby_set_state(pointer, GROUPBY_HAS_TARGET | GROUPBY_HAS_CURRENT);
                for value in old {
                    dec_ref_bits(py, value);
                }
            }
            ObjectShapeId::ItertoolsProduct => {
                let data_pointer = product_data_ptr(pointer);
                let Some(indices) = state_indices(py, state, (*data_pointer).pools_bits.len())
                else {
                    return none;
                };
                let data = &mut *data_pointer;
                let mut values = Vec::new();
                for (&index, &pool) in indices.iter().zip(&data.pools_bits) {
                    let length = seq_read_len(obj_from_bits(pool).as_ptr().unwrap());
                    if length == 0 {
                        data.done = true;
                        return none;
                    }
                    values.push(index.max(0).min(length as i64 - 1) as usize);
                }
                data.indices = values;
                data.first = false;
            }
            ObjectShapeId::ItertoolsCombinations
            | ObjectShapeId::ItertoolsCombinationsWithReplacement => {
                let (n, r) = if shape == ObjectShapeId::ItertoolsCombinations {
                    let data = &*combinations_data_ptr(pointer);
                    (data.n, data.r)
                } else {
                    let data = &*combinations_with_replacement_data_ptr(pointer);
                    (data.n, data.r)
                };
                let Some(indices) = state_indices(py, state, r) else {
                    return none;
                };
                if n == 0 && r != 0 {
                    return raise_exception::<u64>(
                        py,
                        BuiltinExc::IndexError,
                        "tuple index out of range",
                    );
                }
                let indices = indices
                    .into_iter()
                    .enumerate()
                    .map(|(i, index)| {
                        let maximum = if shape == ObjectShapeId::ItertoolsCombinations {
                            i as i64 + n as i64 - r as i64
                        } else {
                            n as i64 - 1
                        };
                        index.min(maximum).max(0) as usize
                    })
                    .collect();
                if shape == ObjectShapeId::ItertoolsCombinations {
                    let data = &mut *combinations_data_ptr(pointer);
                    data.indices = indices;
                    data.first = false;
                } else {
                    let data = &mut *combinations_with_replacement_data_ptr(pointer);
                    data.indices = indices;
                    data.first = false;
                }
            }
            ObjectShapeId::ItertoolsPermutations => {
                let Some(items) = state_items(py, state, Some(2), false) else {
                    return none;
                };
                let data_pointer = permutations_data_ptr(pointer);
                let n = (*data_pointer).n;
                let r = (*data_pointer).r;
                let Some(indices) = state_indices(py, items[0], n) else {
                    return none;
                };
                let Some(cycles) = state_indices(py, items[1], r) else {
                    return none;
                };
                let data = &mut *data_pointer;
                data.indices = indices
                    .into_iter()
                    .map(|index| index.max(0).min(n as i64 - 1) as usize)
                    .collect();
                data.cycles = cycles
                    .into_iter()
                    .enumerate()
                    .map(|(i, index)| index.max(1).min((n - i) as i64) as usize)
                    .collect();
                data.first = false;
            }
            ObjectShapeId::ItertoolsZipLongest => {
                let data = &mut *zip_longest_data_ptr(pointer);
                inc_ref_bits(py, state);
                let old = data.fillvalue_bits;
                data.fillvalue_bits = state;
                dec_ref_bits(py, old);
            }
            _ => {
                return raise_exception::<u64>(
                    py,
                    BuiltinExc::TypeError,
                    "this itertools iterator has no state protocol",
                );
            }
        }
    }
    none
}
