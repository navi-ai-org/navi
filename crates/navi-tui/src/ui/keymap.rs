#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum KeyOutcome {
    Handled,
    Ignored,
    Quit,
}

impl KeyOutcome {
    pub(crate) fn is_handled(self) -> bool {
        !matches!(self, Self::Ignored)
    }

    pub(crate) fn should_quit(self) -> bool {
        matches!(self, Self::Quit)
    }
}
