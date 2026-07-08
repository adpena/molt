use super::*;

pub(super) struct BlockingWaiter {
    pub(super) events: u32,
    pub(super) ready: Mutex<Option<u32>>,
    pub(super) condvar: Condvar,
}

#[cfg(molt_has_net_io)]
#[derive(Default)]
pub(super) struct BlockingWaiterList {
    order: Vec<Arc<BlockingWaiter>>,
    index: HashMap<usize, usize>,
}

#[cfg(molt_has_net_io)]
pub(super) fn blocking_waiter_id(waiter: &Arc<BlockingWaiter>) -> usize {
    Arc::as_ptr(waiter) as usize
}

#[cfg(molt_has_net_io)]
impl BlockingWaiterList {
    pub(super) fn insert(&mut self, waiter: Arc<BlockingWaiter>) -> bool {
        let waiter_id = blocking_waiter_id(&waiter);
        if self.index.contains_key(&waiter_id) {
            return false;
        }
        let next = self.order.len();
        self.order.push(waiter);
        self.index.insert(waiter_id, next);
        true
    }

    fn pop_at(&mut self, idx: usize) -> Option<Arc<BlockingWaiter>> {
        if idx >= self.order.len() {
            return None;
        }
        let removed = self.order.swap_remove(idx);
        self.index.remove(&blocking_waiter_id(&removed));
        if idx < self.order.len() {
            let moved_id = blocking_waiter_id(&self.order[idx]);
            self.index.insert(moved_id, idx);
        }
        Some(removed)
    }

    pub(super) fn remove(&mut self, waiter_id: usize) -> bool {
        let Some(idx) = self.index.get(&waiter_id).copied() else {
            return false;
        };
        self.pop_at(idx).is_some()
    }

    pub(super) fn is_empty(&self) -> bool {
        self.order.is_empty()
    }

    pub(super) fn len(&self) -> usize {
        self.order.len()
    }

    pub(super) fn drain(&mut self) -> Vec<Arc<BlockingWaiter>> {
        self.index.clear();
        std::mem::take(&mut self.order)
    }

    pub(super) fn drain_ready(&mut self, ready_mask: u32) -> Vec<Arc<BlockingWaiter>> {
        let mut ready = Vec::new();
        let mut idx = 0usize;
        while idx < self.order.len() {
            if (self.order[idx].events & ready_mask) != 0 {
                if let Some(waiter) = self.pop_at(idx) {
                    ready.push(waiter);
                }
            } else {
                idx += 1;
            }
        }
        ready
    }
}
