#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ModalStack<M> {
    stack: Vec<M>,
}

impl<M> Default for ModalStack<M> {
    fn default() -> Self {
        Self { stack: Vec::new() }
    }
}

impl<M: Copy + PartialEq> ModalStack<M> {
    pub(crate) fn open(&mut self, modal: M) {
        if self.top() != Some(modal) {
            self.stack.push(modal);
        }
    }

    pub(crate) fn replace(&mut self, modal: Option<M>) {
        self.stack.clear();
        if let Some(modal) = modal {
            self.stack.push(modal);
        }
    }

    pub(crate) fn close(&mut self) -> Option<M> {
        self.stack.pop()
    }

    pub(crate) fn clear(&mut self) {
        self.stack.clear();
    }

    pub(crate) fn top(&self) -> Option<M> {
        self.stack.last().copied()
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
}
