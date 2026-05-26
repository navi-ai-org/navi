#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum UiEffect<M> {
    Quit,
    OpenModal(M),
    ReplaceModal(M),
    CloseModal,
    CloseAllModals,
}
