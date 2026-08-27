use crate::TuiApp;
use crate::dispatch::AsyncEvent;
use crate::runtime::spawn_runtime_task;
use crate::state::ModalKind;
use navi_sdk::NaviUsageReport;

/// Keep long-running turns observable without hammering account endpoints.
const ACCOUNT_USAGE_REFRESH_INTERVAL: std::time::Duration = std::time::Duration::from_secs(30);

pub(crate) fn open_usage_modal(app: &mut TuiApp) {
    crate::keybindings::replace_modal(app, ModalKind::Usage);
    // Apply any stream-cached Hyper balance immediately so the modal never
    // opens empty while the network refresh is in flight.
    apply_live_account_balance(app);
    refresh_usage(app);
}

pub(crate) fn refresh_usage(app: &mut TuiApp) {
    refresh_usage_inner(app, /*quiet*/ false);
}

/// Background refresh used after turns (Crush Hyper credits). Does not clear
/// an existing report while loading, so the UI keeps showing last-known data.
pub(crate) fn refresh_usage_quiet(app: &mut TuiApp) {
    refresh_usage_inner(app, /*quiet*/ true);
}

fn refresh_usage_inner(app: &mut TuiApp, quiet: bool) {
    // Coalesce concurrent fetches: open-modal + after-turn used to race and
    // the loser could overwrite a good report with an empty error payload.
    if app.usage_state.loading {
        if !quiet {
            // Explicit R / open while a fetch is in flight: request one more
            // pass after the current one completes.
            app.usage_state.refresh_pending = true;
        }
        return;
    }
    app.usage_state.loading = true;
    app.usage_state.last_account_refresh_at = Some(std::time::Instant::now());
    app.usage_state.refresh_pending = false;
    if !quiet {
        app.usage_state.error = None;
    }
    let engine = app.engine();
    let tx = app.async_sender();
    spawn_runtime_task(async move {
        let result = engine.usage_report().await.map_err(|err| err.to_string());
        let _ = tx.send(AsyncEvent::UsageLoaded { result });
    });
}

/// Refresh account-backed usage while a turn is active (or the Usage modal is
/// open). Stream usage is not guaranteed to arrive until a request ends, so a
/// periodic balance poll prevents a multi-minute turn from leaving the user
/// without any account-level signal.
pub(crate) fn refresh_account_usage_if_due(app: &mut TuiApp) -> bool {
    if !provider_supports_account_usage(app) || app.usage_state.loading {
        return false;
    }
    let now = std::time::Instant::now();
    if app
        .usage_state
        .last_account_refresh_at
        .is_some_and(|last| now.duration_since(last) < ACCOUNT_USAGE_REFRESH_INTERVAL)
    {
        return false;
    }
    refresh_usage_quiet(app);
    true
}

/// A completed turn should always request a fresh balance, regardless of the
/// periodic refresh interval.
pub(crate) fn refresh_account_usage_after_turn(app: &mut TuiApp) {
    if provider_supports_account_usage(app) && !app.usage_state.loading {
        refresh_usage_quiet(app);
    }
}

fn provider_supports_account_usage(app: &TuiApp) -> bool {
    matches!(
        navi_sdk::canonical_provider_id(&app.loaded_config.config.model.provider),
        "charm-hyper" | "openrouter" | "xai" | "openai"
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn periodic_refresh_waits_for_the_rate_limit_interval() {
        let mut app = crate::tests::test_app("");
        app.is_loading = true;
        app.usage_state.last_account_refresh_at = Some(std::time::Instant::now());

        assert!(!refresh_account_usage_if_due(&mut app));
        assert!(!app.usage_state.loading);
    }
}

/// Pull the process-wide stream/HTTP Hypercredit cache into TUI state and
/// ensure the Usage modal has a non-empty report with a Balance row.
///
/// Called on every `UsageReported` (live), when opening the modal, and after
/// account usage loads — so remaining credits never flash as missing when the
/// stream already told us the balance.
pub(crate) fn apply_live_account_balance(app: &mut TuiApp) {
    let provider = app.loaded_config.config.model.provider.as_str();
    let canonical = navi_sdk::canonical_provider_id(provider);
    if canonical != "charm-hyper" {
        return;
    }
    let Some(balance) = navi_sdk::peek_hypercredit_balance() else {
        return;
    };
    // Mirror the stream sticky policy in the UI: never paint `◆ 0` over a
    // known positive remaining just because a mid-turn usage chunk said 0.
    if balance == 0.0
        && app
            .usage_state
            .remaining_credits
            .is_some_and(|prev| prev > 0.0)
    {
        return;
    }
    app.usage_state.remaining_credits = Some(balance);
    app.usage_state.remaining_credit_unit = Some("hypercredits".into());
    ensure_hypercredit_report(app, balance, "stream-usage");
}

/// If the modal has no useful account report yet, synthesize one from the
/// known Hypercredit balance so the user never sees "no credits" while a
/// background refresh is still running.
fn ensure_hypercredit_report(app: &mut TuiApp, balance: f64, source: &str) {
    let needs_report = match app.usage_state.report.as_ref() {
        None => true,
        Some(r) => {
            r.details.is_empty()
                || !r
                    .details
                    .iter()
                    .any(|d| d.label.eq_ignore_ascii_case("Balance"))
        }
    };
    if !needs_report {
        // Still refresh the Balance value on an existing report so the modal
        // stays current after each turn without waiting for HTTP.
        if let Some(report) = app.usage_state.report.as_mut() {
            let formatted = navi_sdk::format_hypercredits(balance);
            let usd = navi_sdk::hypercredits_to_usd(balance);
            for detail in &mut report.details {
                if detail.label.eq_ignore_ascii_case("Balance") {
                    detail.value = format!("◆ {formatted} Hypercredits");
                } else if detail.label.eq_ignore_ascii_case("Balance (USD)") {
                    detail.value = format!("≈ ${usd:.2}  (1 Hypercredit = $0.05)");
                }
            }
            report.limit_reached_kind = if balance <= 0.0 {
                Some("credits_depleted".into())
            } else {
                None
            };
            if report.source.starts_with("charm-hyper") {
                report.source = format!("charm-hyper-{source}");
            }
        }
        return;
    }

    let provider_id = app.loaded_config.config.model.provider.clone();
    let provider_label = app
        .loaded_config
        .config
        .providers
        .iter()
        .find(|p| p.id == provider_id)
        .map(|p| p.label.clone())
        .unwrap_or_else(|| "Charm Hyper".into());
    let formatted = navi_sdk::format_hypercredits(balance);
    let usd = navi_sdk::hypercredits_to_usd(balance);
    app.usage_state.report = Some(NaviUsageReport {
        provider_id,
        provider_label,
        plan_type: Some("hypercredits".into()),
        limit_reached_kind: if balance <= 0.0 {
            Some("credits_depleted".into())
        } else {
            None
        },
        limits: Vec::new(),
        source: format!("charm-hyper-{source}"),
        notes: Some(
            "Charm Hyper remaining Hypercredits from the last stream usage payload (usage.remaining.hypercredits)."
                .into(),
        ),
        details: vec![
            navi_sdk::NaviUsageDetail {
                label: "Balance".into(),
                value: format!("◆ {formatted} Hypercredits"),
            },
            navi_sdk::NaviUsageDetail {
                label: "Balance (USD)".into(),
                value: format!("≈ ${usd:.2}  (1 Hypercredit = $0.05)"),
            },
            navi_sdk::NaviUsageDetail {
                label: "Billing".into(),
                value: "Prepaid Hypercredits — session spend is estimated from list rates and converted to credits.".into(),
            },
        ],
    });
    app.usage_state.error = None;
}
