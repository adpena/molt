use crate::{
    MoltObject, PyToken, alloc_dict_with_pairs, alloc_list, alloc_tuple, dec_ref_bits,
    dict_get_in_place, dict_set_in_place, exception_pending, obj_from_bits, ptr_from_bits,
    raise_exception, string_obj_to_owned, to_i64,
};
use std::collections::HashMap;
use std::sync::{Arc, Mutex};

const NODE_OUT: i64 = -1;
const NODE_DONE: i64 = -2;

#[derive(Clone)]
struct NodeInfo {
    node_bits: u64,
    npredecessors: i64,
    successors: Vec<usize>,
}

struct GraphState {
    node_map_bits: u64,
    nodes: Vec<NodeInfo>,
    ready_nodes: Option<Vec<usize>>,
    npassedout: usize,
    nfinished: usize,
    freed: bool,
}

impl GraphState {
    fn new(_py: &PyToken<'_>) -> Option<Self> {
        let dict_ptr = alloc_dict_with_pairs(_py, &[]);
        if dict_ptr.is_null() {
            return None;
        }
        Some(Self {
            node_map_bits: MoltObject::from_ptr(dict_ptr).bits(),
            nodes: Vec::new(),
            ready_nodes: None,
            npassedout: 0,
            nfinished: 0,
            freed: false,
        })
    }

    fn clear_refs(&mut self, _py: &PyToken<'_>) {
        if self.freed {
            return;
        }
        self.freed = true;
        for node in self.nodes.drain(..) {
            dec_ref_bits(_py, node.node_bits);
        }
        dec_ref_bits(_py, self.node_map_bits);
        self.node_map_bits = 0;
    }

    fn node_map_ptr(&self) -> Option<*mut u8> {
        obj_from_bits(self.node_map_bits).as_ptr()
    }

    fn lookup_node_index(&self, _py: &PyToken<'_>, node_bits: u64) -> Result<Option<usize>, u64> {
        let dict_ptr = self
            .node_map_ptr()
            .ok_or_else(|| raise_exception::<u64>(_py, "RuntimeError", "graph state lost"))?;
        let found = unsafe { dict_get_in_place(_py, dict_ptr, node_bits) };
        if exception_pending(_py) {
            return Err(MoltObject::none().bits());
        }
        let Some(idx_bits) = found else {
            return Ok(None);
        };
        let Some(idx) = to_i64(obj_from_bits(idx_bits)) else {
            return Err(raise_exception::<_>(
                _py,
                "RuntimeError",
                "graph index corrupted",
            ));
        };
        if idx < 0 {
            return Err(raise_exception::<_>(
                _py,
                "RuntimeError",
                "graph index corrupted",
            ));
        }
        Ok(Some(idx as usize))
    }

    fn get_or_create_index(&mut self, _py: &PyToken<'_>, node_bits: u64) -> Result<usize, u64> {
        if let Some(idx) = self.lookup_node_index(_py, node_bits)? {
            return Ok(idx);
        }
        let dict_ptr = self
            .node_map_ptr()
            .ok_or_else(|| raise_exception::<u64>(_py, "RuntimeError", "graph state lost"))?;
        let idx = self.nodes.len();
        self.nodes.push(NodeInfo {
            node_bits,
            npredecessors: 0,
            successors: Vec::new(),
        });
        crate::inc_ref_bits(_py, node_bits);
        let idx_bits = MoltObject::from_int(idx as i64).bits();
        unsafe {
            dict_set_in_place(_py, dict_ptr, node_bits, idx_bits);
        }
        if exception_pending(_py) {
            if let Some(node) = self.nodes.pop() {
                dec_ref_bits(_py, node.node_bits);
            }
            return Err(MoltObject::none().bits());
        }
        Ok(idx)
    }
}

struct GraphHandle {
    state: Mutex<GraphState>,
}

impl GraphHandle {
    fn new(_py: &PyToken<'_>) -> Option<Self> {
        GraphState::new(_py).map(|state| Self {
            state: Mutex::new(state),
        })
    }
}

fn graph_from_bits(bits: u64) -> Option<Arc<GraphHandle>> {
    let ptr = ptr_from_bits(bits);
    if ptr.is_null() {
        return None;
    }
    unsafe {
        let arc = Arc::from_raw(ptr as *const GraphHandle);
        let cloned = arc.clone();
        let _ = Arc::into_raw(arc);
        Some(cloned)
    }
}

fn node_repr(_py: &PyToken<'_>, node_bits: u64) -> Result<String, u64> {
    let repr_bits = crate::molt_repr_from_obj(node_bits);
    if exception_pending(_py) {
        return Err(MoltObject::none().bits());
    }
    let repr =
        string_obj_to_owned(obj_from_bits(repr_bits)).unwrap_or_else(|| "<unrepr>".to_string());
    dec_ref_bits(_py, repr_bits);
    Ok(repr)
}

fn build_cycle_list(_py: &PyToken<'_>, nodes: &[NodeInfo], cycle: &[usize]) -> u64 {
    let mut out_bits = Vec::with_capacity(cycle.len());
    for &idx in cycle {
        out_bits.push(nodes[idx].node_bits);
    }
    let ptr = alloc_list(_py, out_bits.as_slice());
    MoltObject::from_ptr(ptr).bits()
}

fn find_cycle(nodes: &[NodeInfo]) -> Option<Vec<usize>> {
    let mut seen = vec![false; nodes.len()];
    let mut stack: Vec<usize> = Vec::new();
    let mut itstack: Vec<usize> = Vec::new();
    let mut node2stacki: HashMap<usize, usize> = HashMap::new();

    for start in 0..nodes.len() {
        if seen[start] {
            continue;
        }
        let mut node = start;
        loop {
            if seen[node] {
                if let Some(&stack_idx) = node2stacki.get(&node) {
                    let mut cycle = stack[stack_idx..].to_vec();
                    cycle.push(node);
                    return Some(cycle);
                }
            } else {
                seen[node] = true;
                itstack.push(0);
                node2stacki.insert(node, stack.len());
                stack.push(node);
            }

            while let Some(&top) = stack.last() {
                let succs = &nodes[top].successors;
                let top_idx = itstack.last_mut().expect("itstack out of sync");
                if *top_idx < succs.len() {
                    node = succs[*top_idx];
                    *top_idx += 1;
                    break;
                }
                node2stacki.remove(&top);
                stack.pop();
                itstack.pop();
            }
            if stack.is_empty() {
                break;
            }
        }
    }
    None
}

fn prepare_graph(_py: &PyToken<'_>, state: &mut GraphState) -> Result<Option<Vec<usize>>, u64> {
    if state.ready_nodes.is_some() {
        return Err(raise_exception::<_>(
            _py,
            "ValueError",
            "cannot prepare() more than once",
        ));
    }
    let ready = state
        .nodes
        .iter()
        .enumerate()
        .filter_map(|(idx, node)| {
            if node.npredecessors == 0 {
                Some(idx)
            } else {
                None
            }
        })
        .collect::<Vec<_>>();
    state.ready_nodes = Some(ready);
    Ok(find_cycle(&state.nodes))
}

fn graph_ready_tuple(_py: &PyToken<'_>, ready: &[usize], nodes: &[NodeInfo]) -> u64 {
    let mut out = Vec::with_capacity(ready.len());
    for &idx in ready {
        out.push(nodes[idx].node_bits);
    }
    let ptr = alloc_tuple(_py, out.as_slice());
    MoltObject::from_ptr(ptr).bits()
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_graphlib_new() -> u64 {
    crate::with_gil_entry!(_py, {
        let Some(handle) = GraphHandle::new(_py) else {
            return MoltObject::none().bits();
        };
        let arc = Arc::new(handle);
        let raw = Arc::into_raw(arc) as *mut u8;
        crate::bits_from_ptr(raw)
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_graphlib_add(handle_bits: u64, node_bits: u64, preds_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let Some(handle) = graph_from_bits(handle_bits) else {
            return raise_exception::<_>(_py, "TypeError", "invalid graph handle");
        };
        let mut state = handle.state.lock().unwrap();
        if state.ready_nodes.is_some() {
            return raise_exception::<_>(
                _py,
                "ValueError",
                "Nodes cannot be added after a call to prepare()",
            );
        }
        let node_idx = match state.get_or_create_index(_py, node_bits) {
            Ok(idx) => idx,
            Err(err) => return err,
        };

        let preds_obj = obj_from_bits(preds_bits);
        let preds_ptr = match preds_obj.as_ptr() {
            Some(ptr) => ptr,
            None => return raise_exception::<_>(_py, "TypeError", "predecessors must be a tuple"),
        };
        unsafe {
            let type_id = crate::object_type_id(preds_ptr);
            if type_id != crate::TYPE_ID_TUPLE && type_id != crate::TYPE_ID_LIST {
                return raise_exception::<_>(_py, "TypeError", "predecessors must be a tuple");
            }
            let elems = crate::seq_vec_ref(preds_ptr);
            let pred_count = elems.len();
            if pred_count > 0 {
                state.nodes[node_idx].npredecessors += pred_count as i64;
            }
            for &pred_bits in elems.iter() {
                let pred_idx = match state.get_or_create_index(_py, pred_bits) {
                    Ok(idx) => idx,
                    Err(err) => return err,
                };
                state.nodes[pred_idx].successors.push(node_idx);
            }
        }
        MoltObject::none().bits()
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_graphlib_prepare(handle_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let Some(handle) = graph_from_bits(handle_bits) else {
            return raise_exception::<_>(_py, "TypeError", "invalid graph handle");
        };
        let mut state = handle.state.lock().unwrap();
        let cycle = match prepare_graph(_py, &mut state) {
            Ok(cycle) => cycle,
            Err(err) => return err,
        };
        if let Some(cycle) = cycle {
            return build_cycle_list(_py, &state.nodes, &cycle);
        }
        MoltObject::none().bits()
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_graphlib_get_ready(handle_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let Some(handle) = graph_from_bits(handle_bits) else {
            return raise_exception::<_>(_py, "TypeError", "invalid graph handle");
        };
        let mut state = handle.state.lock().unwrap();
        let ready = match state.ready_nodes.as_ref() {
            Some(nodes) => nodes.clone(),
            None => {
                return raise_exception::<_>(_py, "ValueError", "prepare() must be called first");
            }
        };
        for &idx in ready.iter() {
            state.nodes[idx].npredecessors = NODE_OUT;
        }
        if let Some(nodes) = state.ready_nodes.as_mut() {
            nodes.clear();
        }
        state.npassedout += ready.len();
        graph_ready_tuple(_py, &ready, &state.nodes)
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_graphlib_is_active(handle_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let Some(handle) = graph_from_bits(handle_bits) else {
            return raise_exception::<_>(_py, "TypeError", "invalid graph handle");
        };
        let state = handle.state.lock().unwrap();
        let Some(ready_nodes) = state.ready_nodes.as_ref() else {
            return raise_exception::<_>(_py, "ValueError", "prepare() must be called first");
        };
        MoltObject::from_bool(state.nfinished < state.npassedout || !ready_nodes.is_empty()).bits()
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_graphlib_done(handle_bits: u64, nodes_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let Some(handle) = graph_from_bits(handle_bits) else {
            return raise_exception::<_>(_py, "TypeError", "invalid graph handle");
        };
        let mut state = handle.state.lock().unwrap();
        let ready_nodes_present = state.ready_nodes.is_some();
        if !ready_nodes_present {
            return raise_exception::<_>(_py, "ValueError", "prepare() must be called first");
        }
        let mut newly_ready: Vec<usize> = Vec::new();

        let nodes_obj = obj_from_bits(nodes_bits);
        let nodes_ptr = match nodes_obj.as_ptr() {
            Some(ptr) => ptr,
            None => return raise_exception::<_>(_py, "TypeError", "nodes must be a tuple"),
        };
        unsafe {
            let type_id = crate::object_type_id(nodes_ptr);
            if type_id != crate::TYPE_ID_TUPLE && type_id != crate::TYPE_ID_LIST {
                return raise_exception::<_>(_py, "TypeError", "nodes must be a tuple");
            }
            let elems = crate::seq_vec_ref(nodes_ptr);
            for &node_bits in elems.iter() {
                let node_idx = match state.lookup_node_index(_py, node_bits) {
                    Ok(Some(idx)) => idx,
                    Ok(None) => {
                        let repr = match node_repr(_py, node_bits) {
                            Ok(text) => text,
                            Err(err) => return err,
                        };
                        let msg = format!("node {repr} was not added using add()");
                        return raise_exception::<_>(_py, "ValueError", &msg);
                    }
                    Err(err) => return err,
                };
                let stat = state.nodes[node_idx].npredecessors;
                if stat != NODE_OUT {
                    let repr = match node_repr(_py, node_bits) {
                        Ok(text) => text,
                        Err(err) => return err,
                    };
                    if stat >= 0 {
                        let msg = format!("node {repr} was not passed out (still not ready)");
                        return raise_exception::<_>(_py, "ValueError", &msg);
                    }
                    if stat == NODE_DONE {
                        let msg = format!("node {repr} was already marked done");
                        return raise_exception::<_>(_py, "ValueError", &msg);
                    }
                    let msg = format!("node {repr}: unknown status {stat}");
                    return raise_exception::<_>(_py, "AssertionError", &msg);
                }

                state.nodes[node_idx].npredecessors = NODE_DONE;
                let succs = state.nodes[node_idx].successors.clone();
                for succ_idx in succs {
                    let successor_info = &mut state.nodes[succ_idx];
                    successor_info.npredecessors -= 1;
                    if successor_info.npredecessors == 0 {
                        newly_ready.push(succ_idx);
                    }
                }
                state.nfinished += 1;
            }
        }
        if let Some(ready_nodes) = state.ready_nodes.as_mut() {
            ready_nodes.extend(newly_ready);
        }
        MoltObject::none().bits()
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_graphlib_static_order(handle_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let Some(handle) = graph_from_bits(handle_bits) else {
            return raise_exception::<_>(_py, "TypeError", "invalid graph handle");
        };
        let mut state = handle.state.lock().unwrap();
        let cycle = match prepare_graph(_py, &mut state) {
            Ok(cycle) => cycle,
            Err(err) => return err,
        };
        if let Some(cycle) = cycle {
            let cycle_bits = build_cycle_list(_py, &state.nodes, &cycle);
            let tuple_ptr = alloc_tuple(_py, &[MoltObject::from_bool(false).bits(), cycle_bits]);
            return MoltObject::from_ptr(tuple_ptr).bits();
        }

        let mut order: Vec<u64> = Vec::new();
        loop {
            let ready_snapshot = match state.ready_nodes.as_ref() {
                Some(nodes) => nodes.clone(),
                None => {
                    return raise_exception::<_>(
                        _py,
                        "ValueError",
                        "prepare() must be called first",
                    );
                }
            };
            if state.nfinished >= state.npassedout && ready_snapshot.is_empty() {
                break;
            }
            for &idx in ready_snapshot.iter() {
                state.nodes[idx].npredecessors = NODE_OUT;
            }
            if let Some(nodes) = state.ready_nodes.as_mut() {
                nodes.clear();
            }
            state.npassedout += ready_snapshot.len();

            for &idx in ready_snapshot.iter() {
                order.push(state.nodes[idx].node_bits);
            }
            let mut newly_ready: Vec<usize> = Vec::new();
            for &idx in ready_snapshot.iter() {
                state.nodes[idx].npredecessors = NODE_DONE;
                let succs = state.nodes[idx].successors.clone();
                for succ_idx in succs {
                    let successor_info = &mut state.nodes[succ_idx];
                    successor_info.npredecessors -= 1;
                    if successor_info.npredecessors == 0 {
                        newly_ready.push(succ_idx);
                    }
                }
                state.nfinished += 1;
            }
            if let Some(nodes) = state.ready_nodes.as_mut() {
                nodes.extend(newly_ready);
            }
        }

        let order_ptr = alloc_tuple(_py, order.as_slice());
        let tuple_ptr = alloc_tuple(
            _py,
            &[
                MoltObject::from_bool(true).bits(),
                MoltObject::from_ptr(order_ptr).bits(),
            ],
        );
        MoltObject::from_ptr(tuple_ptr).bits()
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_graphlib_drop(handle_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let ptr = ptr_from_bits(handle_bits);
        if ptr.is_null() {
            return MoltObject::none().bits();
        }
        unsafe {
            let arc = Arc::from_raw(ptr as *const GraphHandle);
            if let Ok(mut state) = arc.state.lock() {
                state.clear_refs(_py);
            }
        }
        MoltObject::none().bits()
    })
}
