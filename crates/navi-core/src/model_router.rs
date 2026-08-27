//! Benchmark-driven model routing contracts.

use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ModelScorecard {
    pub provider_id: String,
    pub model: String,
    pub role: ModelRouteRole,
    pub success_rate: f64,
    pub verifier_pass_rate: f64,
    pub tool_call_validity: f64,
    pub cost_per_1k_tokens: f64,
    pub latency_ms: f64,
    pub retry_rate: f64,
    pub unsafe_action_rate: f64,
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ModelRouteRole {
    Planner,
    Router,
    Coder,
    Reviewer,
    VerifierJudge,
    Summarizer,
    MemoryMiner,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ModelRoute {
    pub role: ModelRouteRole,
    pub provider_id: String,
    pub model: String,
    pub score: f64,
    pub fallback: Option<Box<ModelRoute>>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ModelRouter {
    scorecards: BTreeMap<ModelRouteRole, Vec<ModelScorecard>>,
}

impl ModelRouter {
    pub fn add_scorecard(&mut self, scorecard: ModelScorecard) {
        self.scorecards
            .entry(scorecard.role.clone())
            .or_default()
            .push(scorecard);
    }

    pub fn route(&self, role: ModelRouteRole, high_risk: bool) -> Option<ModelRoute> {
        let mut candidates = self.scorecards.get(&role)?.clone();
        candidates.sort_by(|left, right| {
            route_score(right, high_risk)
                .partial_cmp(&route_score(left, high_risk))
                .unwrap_or(std::cmp::Ordering::Equal)
        });
        let best = candidates.first()?;
        let fallback = candidates.get(1).map(|candidate| {
            Box::new(ModelRoute {
                role: role.clone(),
                provider_id: candidate.provider_id.clone(),
                model: candidate.model.clone(),
                score: route_score(candidate, high_risk),
                fallback: None,
            })
        });
        Some(ModelRoute {
            role,
            provider_id: best.provider_id.clone(),
            model: best.model.clone(),
            score: route_score(best, high_risk),
            fallback,
        })
    }
}

fn route_score(card: &ModelScorecard, high_risk: bool) -> f64 {
    let mut score =
        card.success_rate * 35.0 + card.verifier_pass_rate * 30.0 + card.tool_call_validity * 15.0
            - card.retry_rate * 10.0
            - card.unsafe_action_rate * if high_risk { 40.0 } else { 15.0 }
            - (card.cost_per_1k_tokens * 2.0)
            - (card.latency_ms / 10_000.0).min(10.0);
    if high_risk && card.verifier_pass_rate >= 0.95 {
        score += 5.0;
    }
    score
}

#[cfg(test)]
mod tests {
    use super::*;

    fn card(model: &str, unsafe_rate: f64, cost: f64) -> ModelScorecard {
        ModelScorecard {
            provider_id: "p".to_string(),
            model: model.to_string(),
            role: ModelRouteRole::Coder,
            success_rate: 0.9,
            verifier_pass_rate: 0.95,
            tool_call_validity: 0.98,
            cost_per_1k_tokens: cost,
            latency_ms: 1000.0,
            retry_rate: 0.1,
            unsafe_action_rate: unsafe_rate,
        }
    }

    #[test]
    fn high_risk_routes_away_from_unsafe_model() {
        let mut router = ModelRouter::default();
        router.add_scorecard(card("cheap-risky", 0.2, 0.1));
        router.add_scorecard(card("safe", 0.0, 1.0));

        let route = router.route(ModelRouteRole::Coder, true).unwrap();

        assert_eq!(route.model, "safe");
        assert_eq!(route.fallback.unwrap().model, "cheap-risky");
    }
}
