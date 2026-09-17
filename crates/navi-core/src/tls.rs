//! Process-wide TLS setup for the workspace's rustls-based HTTP stack.
//!
//! reqwest is built with `rustls-no-provider` — the only reqwest 0.13 TLS
//! configuration that does not pull in `aws-lc-rs`. With that feature reqwest
//! panics on the first `Client` build unless a process-wide `CryptoProvider`
//! has been installed (see `rustls::crypto::CryptoProvider::get_default`).
//!
//! NAVI standardises on `ring`, so this module installs the ring provider
//! through a `ctor` initializer that runs before `main` in every binary (and
//! test binary) that links `navi-core`. Hosts do not need to call anything.

use std::sync::Once;

static INSTALL: Once = Once::new();

/// Installs the `ring` `CryptoProvider` as the process default, at most once.
///
/// Safe to call from any thread and any number of times; later calls are
/// no-ops. The `ctor` initializer below already runs this before `main`, so
/// explicit calls are only needed by hosts that build reqwest clients in a
/// process that does not link `navi-core`.
pub fn install_default_crypto_provider() {
    INSTALL.call_once(|| {
        // If another component already installed a provider, `install_default`
        // returns `Err` with that provider. That is fine: providers are
        // process-wide and any installed one keeps reqwest working.
        let _ = rustls::crypto::ring::default_provider().install_default();
    });
}

#[ctor::ctor(unsafe)]
fn install_default_crypto_provider_before_main() {
    install_default_crypto_provider();
}
