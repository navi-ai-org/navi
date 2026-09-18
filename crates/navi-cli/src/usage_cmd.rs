//! `navi usage` — token/cache/credit accounting for saved sessions.
//!
//! Aggregates the `UsageReported` events that the runtime already records per
//! model call, so the effect of context/caching changes is measurable instead
//! of guessed. Reports the prompt-cache split (cached vs full-price input) and
//! the list-rate cost, plus what the same traffic would have cost without a
//! prompt cache — the number that shows whether caching is actually working.

use anyhow::{Context, Result};
use navi_core::{
    AgentEvent, LoadedConfig, SessionStore, estimate_token_cost_usd_with_cache,
    model_cache_list_pricing, model_list_pricing, provider_credit_unit, usd_to_provider_credits,
};
use serde_json::Value;

use crate::UsageAction;

#[derive(Debug, Default, Clone)]
struct UsageTotals {
    calls: u64,
    input_tokens: u64,
    output_tokens: u64,
    cache_read_tokens: u64,
    cache_creation_tokens: u64,
    max_input_tokens: u64,
}

impl UsageTotals {
    fn add(&mut self, u: &UsageTotals) {
        self.calls += u.calls;
        self.input_tokens += u.input_tokens;
        self.output_tokens += u.output_tokens;
        self.cache_read_tokens += u.cache_read_tokens;
        self.cache_creation_tokens += u.cache_creation_tokens;
        self.max_input_tokens = self.max_input_tokens.max(u.max_input_tokens);
    }

    fn cache_hit_percent(&self) -> f64 {
        if self.input_tokens == 0 {
            return 0.0;
        }
        100.0 * self.cache_read_tokens as f64 / self.input_tokens as f64
    }

    fn avg_input_tokens(&self) -> u64 {
        self.input_tokens.checked_div(self.calls).unwrap_or(0)
    }
}

#[derive(Debug, Clone)]
struct SessionUsage {
    id: String,
    title: String,
    project: String,
    provider: String,
    model: String,
    totals: UsageTotals,
}

pub fn handle_usage_command(action: UsageAction, loaded_config: &LoadedConfig) -> Result<()> {
    match action {
        UsageAction::Report {
            json,
            limit,
            session,
        } => {
            let sessions = collect_usage(loaded_config, session.as_deref())?;
            if json {
                print_json(&sessions, loaded_config);
            } else {
                print_report(&sessions, loaded_config, limit);
            }
            Ok(())
        }
    }
}

/// Loads usage totals for every saved session (or a single one).
fn collect_usage(loaded_config: &LoadedConfig, only: Option<&str>) -> Result<Vec<SessionUsage>> {
    let store = SessionStore::with_redaction(
        loaded_config.data_dir.clone(),
        loaded_config.config.security.redact_secrets_in_sessions,
    );

    let ids: Vec<String> = match only {
        Some(id) => vec![id.to_string()],
        None => store
            .list_info()
            .into_iter()
            .map(|i| i.id.as_str().to_string())
            .collect(),
    };

    let fallback_provider = loaded_config.config.model.provider.clone();
    let fallback_model = loaded_config.config.model.name.clone();
    let mut out = Vec::new();

    for id in ids {
        let snapshot = match store.load(&id) {
            Ok(snapshot) => snapshot,
            Err(err) => {
                if only.is_some() {
                    return Err(err).with_context(|| format!("failed to load session {id}"));
                }
                continue;
            }
        };

        let mut totals = UsageTotals::default();
        let mut provider = fallback_provider.clone();
        let mut model = fallback_model.clone();

        for event in &snapshot.events {
            match event {
                AgentEvent::UsageReported {
                    input_tokens,
                    output_tokens,
                    cache_creation_tokens,
                    cache_read_tokens,
                } => {
                    totals.calls += 1;
                    totals.input_tokens += input_tokens;
                    totals.output_tokens += output_tokens;
                    totals.cache_creation_tokens += cache_creation_tokens;
                    totals.cache_read_tokens += cache_read_tokens;
                    totals.max_input_tokens = totals.max_input_tokens.max(*input_tokens);
                }
                // HarnessTrace carries the provider/model that served the turn.
                AgentEvent::HarnessTrace(value) => {
                    if let Some((p, m)) = trace_model(value) {
                        provider = p;
                        model = m;
                    }
                }
                _ => {}
            }
        }

        if totals.calls == 0 {
            continue;
        }

        out.push(SessionUsage {
            id: snapshot.id.as_str().to_string(),
            title: snapshot
                .title
                .clone()
                .unwrap_or_else(|| "(untitled)".into()),
            project: snapshot.project.display().to_string(),
            provider,
            model,
            totals,
        });
    }

    out.sort_by(|a, b| b.totals.input_tokens.cmp(&a.totals.input_tokens));
    Ok(out)
}

fn trace_model(value: &Value) -> Option<(String, String)> {
    let provider = value.get("model_provider")?.as_str()?.to_string();
    let model = value.get("model_name")?.as_str()?.to_string();
    if provider.is_empty() || model.is_empty() {
        return None;
    }
    Some((provider, model))
}

/// List-rate cost for one session: (cost, cost_without_cache).
fn session_cost(loaded_config: &LoadedConfig, session: &SessionUsage) -> Option<(f64, f64)> {
    let config = &loaded_config.config;
    let (input_per_1m, output_per_1m) =
        model_list_pricing(config, &session.provider, &session.model)?;
    let (cache_in, cache_write) =
        model_cache_list_pricing(&session.provider).unwrap_or((input_per_1m * 0.1, input_per_1m));

    let t = &session.totals;
    let cost = estimate_token_cost_usd_with_cache(
        t.input_tokens,
        t.output_tokens,
        t.cache_read_tokens,
        t.cache_creation_tokens,
        input_per_1m,
        output_per_1m,
        Some(cache_in),
        Some(cache_write),
    );
    // Same traffic billed as if every prompt token missed the cache.
    let no_cache = estimate_token_cost_usd_with_cache(
        t.input_tokens,
        t.output_tokens,
        0,
        t.cache_creation_tokens,
        input_per_1m,
        output_per_1m,
        Some(cache_in),
        Some(cache_write),
    );
    Some((cost, no_cache))
}

fn print_report(sessions: &[SessionUsage], loaded_config: &LoadedConfig, limit: usize) {
    if sessions.is_empty() {
        println!(
            "No usage recorded yet in {}",
            loaded_config.data_dir.join("sessions").display()
        );
        return;
    }

    let mut grand = UsageTotals::default();
    let mut grand_cost = 0.0_f64;
    let mut grand_no_cache = 0.0_f64;
    let mut credit_unit: Option<&'static str> = None;

    println!(
        "{:<28} {:<18} {:>7} {:>13} {:>12} {:>11} {:>9}",
        "session", "model", "calls", "input", "cache_read", "avg_ctx", "hit%"
    );
    for session in sessions.iter().take(limit) {
        let t = &session.totals;
        let cost = session_cost(loaded_config, session);
        if let Some((c, n)) = cost {
            grand_cost += c;
            grand_no_cache += n;
        }
        if credit_unit.is_none() {
            credit_unit = provider_credit_unit(&session.provider);
        }
        println!(
            "{:<28} {:<18} {:>7} {:>13} {:>12} {:>11} {:>8.1}%",
            truncate(&session.title, 28),
            truncate(&session.model, 18),
            t.calls,
            short(t.input_tokens),
            short(t.cache_read_tokens),
            short(t.avg_input_tokens()),
            t.cache_hit_percent(),
        );
    }
    for session in sessions {
        grand.add(&session.totals);
    }

    println!();
    println!("sessions: {}", sessions.len());
    println!("model calls: {}", grand.calls);
    println!(
        "input tokens: {} ({} cached, {} full price)",
        short(grand.input_tokens),
        short(grand.cache_read_tokens),
        short(grand.input_tokens.saturating_sub(grand.cache_read_tokens)),
    );
    println!("output tokens: {}", short(grand.output_tokens));
    println!("cache hit rate: {:.1}%", grand.cache_hit_percent());
    println!(
        "largest single prompt: {} tokens (avg {})",
        short(grand.max_input_tokens),
        short(grand.avg_input_tokens())
    );
    if grand_cost > 0.0 {
        println!();
        println!("list-rate cost: ${grand_cost:.4}");
        println!(
            "same traffic without a prompt cache: ${grand_no_cache:.4} ({:.0}x more)",
            if grand_cost > 0.0 {
                grand_no_cache / grand_cost
            } else {
                0.0
            }
        );
        if let Some(unit) = credit_unit {
            for session in sessions {
                if let Some(credits) = usd_to_provider_credits(&session.provider, grand_cost) {
                    println!(
                        "≈ {:.0} {} at {}'s documented rate",
                        credits, unit, session.provider
                    );
                    break;
                }
            }
        }
    }
    if let Some(cap) = loaded_config.config.harness.context_cap_tokens {
        println!();
        println!("context cap: {} tokens", short(cap));
    } else {
        println!();
        println!(
            "context cap: none — set `harness.context_cap_tokens` to bound per-step prompt spend"
        );
    }
}

fn print_json(sessions: &[SessionUsage], loaded_config: &LoadedConfig) {
    let rows: Vec<Value> = sessions
        .iter()
        .map(|s| {
            let t = &s.totals;
            let (cost, no_cache) = session_cost(loaded_config, s).unwrap_or((0.0, 0.0));
            serde_json::json!({
                "session_id": s.id,
                "title": s.title,
                "project": s.project,
                "provider": s.provider,
                "model": s.model,
                "calls": t.calls,
                "input_tokens": t.input_tokens,
                "output_tokens": t.output_tokens,
                "cache_read_tokens": t.cache_read_tokens,
                "cache_creation_tokens": t.cache_creation_tokens,
                "cache_hit_percent": s.totals.cache_hit_percent(),
                "avg_input_tokens": s.totals.avg_input_tokens(),
                "max_input_tokens": t.max_input_tokens,
                "cost_usd": cost,
                "cost_usd_without_cache": no_cache,
            })
        })
        .collect();
    let payload = serde_json::json!({
        "context_cap_tokens": loaded_config.config.harness.context_cap_tokens,
        "sessions": rows,
    });
    match serde_json::to_string_pretty(&payload) {
        Ok(text) => println!("{text}"),
        Err(err) => eprintln!("failed to encode usage report: {err}"),
    }
}

fn short(tokens: u64) -> String {
    if tokens >= 1_000_000 {
        format!("{:.1}M", tokens as f64 / 1_000_000.0)
    } else if tokens >= 1_000 {
        format!("{:.0}k", tokens as f64 / 1_000.0)
    } else {
        tokens.to_string()
    }
}

fn truncate(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        return s.to_string();
    }
    let mut out: String = s.chars().take(max.saturating_sub(1)).collect();
    out.push('…');
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cache_hit_percent_uses_inclusive_input_tokens() {
        let totals = UsageTotals {
            calls: 2,
            input_tokens: 1_000,
            output_tokens: 10,
            cache_read_tokens: 900,
            cache_creation_tokens: 0,
            max_input_tokens: 600,
        };
        assert_eq!(totals.cache_hit_percent(), 90.0);
        assert_eq!(totals.avg_input_tokens(), 500);
    }

    #[test]
    fn cache_hit_percent_is_zero_without_input() {
        assert_eq!(UsageTotals::default().cache_hit_percent(), 0.0);
    }

    #[test]
    fn short_formats_thousands_and_millions() {
        assert_eq!(short(999), "999");
        assert_eq!(short(12_400), "12k");
        assert_eq!(short(1_808_063_314), "1808.1M");
    }
}
