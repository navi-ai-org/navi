# 18 — Credentials under vault story (Agares)

**What to build:** Provider API keys and Navi credential files used by Agares live under the app/vault data directory with an explicit security story—no second ad-hoc secret store in the monorepo or user home defaults that bypass Agares data dir policy.

**Blocked by:** 09 — Vault-scoped Navi engine lifecycle

**Repo:** Agares (policy) + navi (data_dir / credential path already config-driven)

**Status:** ready-for-agent

## Acceptance criteria

- [ ] Credential store path resolves under Agares-chosen data dir when engine is vault-scoped
- [ ] Document which secrets are plaintext at rest vs covered by OS permissions
- [ ] Optional: future work noted for encrypting credentials with vault key (out of scope unless specified)
- [ ] UI for set/delete provider key uses Navi APIs and does not log secrets
- [ ] Lock vault does not leave keys loaded in disposed engine memory longer than process lifetime allows
