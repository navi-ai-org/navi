//! Binary min-heap with decrease-key style updates by task id.

use std::collections::HashMap;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Task {
    pub id: u64,
    pub priority: i64,
}

#[derive(Default)]
pub struct PriorityQueue {
    heap: Vec<Task>,
    /// id → index in heap
    index: HashMap<u64, usize>,
}

impl PriorityQueue {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn len(&self) -> usize {
        self.heap.len()
    }

    pub fn is_empty(&self) -> bool {
        self.heap.is_empty()
    }

    pub fn push(&mut self, task: Task) {
        if self.index.contains_key(&task.id) {
            // update existing
            self.decrease_or_increase(task.id, task.priority);
            return;
        }
        let i = self.heap.len();
        self.heap.push(task.clone());
        self.index.insert(task.id, i);
        self.sift_up(i);
    }

    pub fn pop_min(&mut self) -> Option<Task> {
        if self.heap.is_empty() {
            return None;
        }
        let min = self.heap[0].clone();
        let last = self.heap.pop().unwrap();
        self.index.remove(&min.id);
        if !self.heap.is_empty() {
            let id = last.id;
            self.heap[0] = last;
            self.index.insert(id, 0);
            self.sift_down(0);
        }
        Some(min)
    }

    pub fn peek_min(&self) -> Option<&Task> {
        self.heap.first()
    }

    pub fn decrease_or_increase(&mut self, id: u64, new_priority: i64) {
        let Some(&i) = self.index.get(&id) else {
            return;
        };
        let old = self.heap[i].priority;
        self.heap[i].priority = new_priority;
        // TODO(fix): always sift_down even when priority decreases (should sift_up)
        // Correct: if new < old { sift_up } else { sift_down }
        if new_priority > old {
            self.sift_up(i); // inverted!
        } else {
            self.sift_down(i); // inverted!
        }
    }

    fn sift_up(&mut self, mut i: usize) {
        while i > 0 {
            let p = (i - 1) / 2;
            // TODO(fix): inverted comparison — behaves like a max-heap sift.
            // Min-heap must stop when child.priority >= parent.priority
            // (i.e. continue while child is *strictly smaller*).
            if self.heap[i].priority <= self.heap[p].priority {
                break;
            }
            self.swap(i, p);
            i = p;
        }
    }

    fn sift_down(&mut self, mut i: usize) {
        let n = self.heap.len();
        loop {
            let l = 2 * i + 1;
            let r = 2 * i + 2;
            let mut best = i;
            if l < n && self.heap[l].priority < self.heap[best].priority {
                best = l;
            }
            if r < n && self.heap[r].priority < self.heap[best].priority {
                best = r;
            }
            if best == i {
                break;
            }
            self.swap(i, best);
            i = best;
        }
    }

    fn swap(&mut self, a: usize, b: usize) {
        self.heap.swap(a, b);
        let id_a = self.heap[a].id;
        let id_b = self.heap[b].id;
        self.index.insert(id_a, a);
        self.index.insert(id_b, b);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn t(id: u64, p: i64) -> Task {
        Task { id, priority: p }
    }

    #[test]
    fn pops_in_priority_order() {
        let mut q = PriorityQueue::new();
        q.push(t(1, 5));
        q.push(t(2, 1));
        q.push(t(3, 3));
        assert_eq!(q.pop_min().unwrap().id, 2);
        assert_eq!(q.pop_min().unwrap().id, 3);
        assert_eq!(q.pop_min().unwrap().id, 1);
        assert!(q.pop_min().is_none());
    }

    #[test]
    fn decrease_key_bubbles_up() {
        let mut q = PriorityQueue::new();
        q.push(t(1, 10));
        q.push(t(2, 20));
        q.push(t(3, 30));
        q.decrease_or_increase(3, 0);
        assert_eq!(q.peek_min().unwrap().id, 3);
        assert_eq!(q.pop_min().unwrap().priority, 0);
    }

    #[test]
    fn increase_key_sifts_down() {
        let mut q = PriorityQueue::new();
        q.push(t(1, 1));
        q.push(t(2, 2));
        q.push(t(3, 3));
        q.decrease_or_increase(1, 100);
        assert_eq!(q.pop_min().unwrap().id, 2);
        assert_eq!(q.pop_min().unwrap().id, 3);
        assert_eq!(q.pop_min().unwrap().id, 1);
    }

    #[test]
    fn update_same_id_replaces_priority() {
        let mut q = PriorityQueue::new();
        q.push(t(7, 50));
        q.push(t(7, 1));
        assert_eq!(q.len(), 1);
        assert_eq!(q.peek_min().unwrap().priority, 1);
    }

    #[test]
    fn many_inserts_stay_ordered() {
        let mut q = PriorityQueue::new();
        for i in (0..20).rev() {
            q.push(t(i, i as i64));
        }
        let mut prev = i64::MIN;
        while let Some(task) = q.pop_min() {
            assert!(task.priority >= prev);
            prev = task.priority;
        }
    }
}
