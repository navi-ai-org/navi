# NAVI Plugin System Release Gates

## Merge Gate

A PR can merge only if:
- cargo fmt passes
- cargo clippy passes
- cargo test passes
- traceability matrix updated for changed requirements
- tests added for changed requirements
- no new unsafe plugin path exposed
- no direct fs/network/env access added to plugin runtime
- no security default weakened

## Security Gate

A plugin system release is BLOCKED if:
- community plugin can run native in-process
- plugin can read env directly
- plugin can access network without HTTP broker
- plugin can access filesystem without FS broker
- plugin can register model-facing free-form tool description
- plugin can shadow built-in tool
- plugin can bypass reconsent on new capabilities
- any red-team test fails
- any security default is set to allow-all
- audit log is not functional

## Milestone Gate

A milestone is COMPLETE only if:
- all exit criteria from implementation plan are met
- all tests for that milestone pass
- traceability matrix is updated
- no open security TODOs remain
- docs are updated

## Red-Team Gate

Before any public release:
- all 10 red-team fixtures must pass
- red-team results must be documented
- any new attack vector discovered must have a test added
- residual risks must be documented in threat model
