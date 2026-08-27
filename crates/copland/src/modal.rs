#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ModalStack<M> {
    stack: Vec<M>,
}

impl<M> Default for ModalStack<M> {
    fn default() -> Self {
        Self { stack: Vec::new() }
    }
}

impl<M: Copy + PartialEq> ModalStack<M> {
    pub fn open(&mut self, modal: M) {
        if self.top() != Some(modal) {
            self.stack.push(modal);
        }
    }

    pub fn replace(&mut self, modal: Option<M>) {
        self.stack.clear();
        if let Some(modal) = modal {
            self.stack.push(modal);
        }
    }

    pub fn close(&mut self) -> Option<M> {
        self.stack.pop()
    }

    pub fn clear(&mut self) {
        self.stack.clear();
    }

    pub fn top(&self) -> Option<M> {
        self.stack.last().copied()
    }

    pub fn is_active(&self) -> bool {
        !self.stack.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    enum Modal {
        Parent,
        Child,
    }

    #[test]
    fn stack_returns_to_parent_modal() {
        let mut stack = ModalStack::default();

        stack.open(Modal::Parent);
        stack.open(Modal::Child);
        assert_eq!(stack.top(), Some(Modal::Child));

        assert_eq!(stack.close(), Some(Modal::Child));
        assert_eq!(stack.top(), Some(Modal::Parent));
    }

    #[test]
    fn replace_discards_previous_context() {
        let mut stack = ModalStack::default();

        stack.open(Modal::Parent);
        stack.open(Modal::Child);
        stack.replace(Some(Modal::Parent));

        assert_eq!(stack.close(), Some(Modal::Parent));
        assert_eq!(stack.top(), None);
    }

    #[test]
    fn is_active_tracks_modal_presence() {
        let mut stack = ModalStack::default();

        assert!(!stack.is_active());
        stack.open(Modal::Parent);
        assert!(stack.is_active());
        stack.close();
        assert!(!stack.is_active());
    }
}
