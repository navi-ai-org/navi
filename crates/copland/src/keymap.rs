#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum KeyOutcome {
    Handled,
    Ignored,
    Quit,
}

impl KeyOutcome {
    pub fn is_handled(self) -> bool {
        !matches!(self, Self::Ignored)
    }

    pub fn should_quit(self) -> bool {
        matches!(self, Self::Quit)
    }
}
