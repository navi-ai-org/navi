# tool-select-edit fixture

Tiny multi-file crate. The bug is a wrong constant in `src/config.rs`
referenced by `src/lib.rs`. Agents should locate the source of the failing
test (read/grep), edit the right file, and re-run tests — not rewrite the
whole crate.
