#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UiEffect<M> {
    Quit,
    OpenModal(M),
    ReplaceModal(M),
    CloseModal,
    CloseAllModals,
}
