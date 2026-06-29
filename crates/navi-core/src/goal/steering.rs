use crate::goal::types::SessionGoal;

/// Builds a continuation steering prompt for injection when the agent is idle
/// but has an active goal.
pub fn build_continuation_prompt(goal: &SessionGoal) -> String {
    format!(
        "\
# Active Thread Goal

<objective>
{objective}
</objective>

**Goal Status**: {status}
**Tokens Used**: {tokens_used} / {budget}
**Time Elapsed**: {time_seconds}s

## Instructions

Continue working toward the active thread goal above. You are in an auto-continuation
turn — the user expects you to make progress without additional prompting.

### Completion Audit Rules
Before marking this goal as complete:
1. The objective must be fully satisfied and verified.
2. All tests must pass and build must succeed.
3. The final deliverable must be ready for end-user use.
4. Do not call update_goal(complete) unless all verification steps pass.

### Blocked Audit Rules
If the same blocker occurs for 3+ consecutive turns, call update_goal(blocked) with
the specific blocker description. A blocker is a persistent error that prevents
forward progress (e.g., missing dependency, API error, permission denied).
Temporary failures like network flakes are not blockers.
",
        objective = goal.objective,
        status = goal.status.as_str(),
        tokens_used = goal.tokens_used,
        budget = goal
            .token_budget
            .map(|b| b.to_string())
            .unwrap_or_else(|| "unlimited".to_string()),
        time_seconds = goal.time_used_seconds,
    )
}

/// Builds a budget-limit steering prompt when the token budget is exceeded.
pub fn build_budget_limit_prompt(goal: &SessionGoal) -> String {
    format!(
        "\
# Budget Limit Reached

The token budget for your current goal has been exceeded.

**Budget**: {budget} tokens
**Used**: {tokens_used} tokens
**Goal**: {objective}

You must finish your work immediately. Do not start new sub-tasks or exploratory
reading. Finalize any in-progress work, run verification, and produce a summary
of what was accomplished versus what remains.

The goal status has been set to `budget_limited`. No further auto-continuation
turns will be triggered.
",
        budget = goal.token_budget.map(|b| b.to_string()).unwrap_or_default(),
        tokens_used = goal.tokens_used,
        objective = goal.objective,
    )
}

/// Builds an objective-updated steering prompt when the user changes the goal.
pub fn build_objective_updated_prompt(goal: &SessionGoal) -> String {
    format!(
        "\
# Objective Updated

The user has updated the thread goal objective. Your new objective is:

<objective>
{objective}
</objective>

Previous progress may still be relevant. Adapt your approach to the new objective.
The goal is now `active` and you should continue working.
",
        objective = goal.objective,
    )
}
