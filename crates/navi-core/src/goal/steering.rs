use crate::goal::types::SessionGoal;

/// Builds a continuation steering prompt for injection when the agent is idle
/// but has an active goal.
pub fn build_continuation_prompt(goal: &SessionGoal) -> String {
    let checklist_section = if goal.checklist.is_empty() {
        "\n## ⚠ NO CHECKLIST DEFINED\n\
        You have not yet created a task checklist for this goal. Your FIRST action in this turn\n\
        MUST be to call `update_goal_checklist(action=\"set\", tasks=[...])` with a concrete,\n\
        verifiable decomposition of the objective. Each task should be small enough to verify\n\
        independently (e.g. \"run cargo test -p navi-core\", \"add X function to Y file\", etc).\n\
        Do NOT attempt to mark the goal complete without a checklist.\n"
            .to_string()
    } else {
        let total = goal.checklist.len();
        let finished = goal.finished_count();
        let verified = goal.verified_count();
        let task_lines: Vec<String> = goal
            .checklist
            .iter()
            .map(|t| {
                let marker = match t.status {
                    crate::goal::types::TaskStatus::Verified => "✓",
                    crate::goal::types::TaskStatus::Skipped => "⊘",
                    crate::goal::types::TaskStatus::InProgress => "▶",
                    crate::goal::types::TaskStatus::Done => "○",
                    crate::goal::types::TaskStatus::Pending => "·",
                };
                let ver = if t.verified {
                    " (verified)".to_string()
                } else if let Some(ref v) = t.verification {
                    format!(" (verification: {})", v)
                } else {
                    String::new()
                };
                format!("  {} [{}] {}{}", marker, t.status, t.description, ver)
            })
            .collect();

        format!(
            "\n## Checklist Progress: {verified}/{total} verified ({finished} finished)\n\
            {tasks}\n\n\
            ### Next Action\n\
            Work on the next unfinished task. After implementing it, run verification\n\
            (tests, build, lint) and mark it `verified` with `update_goal_checklist`.\n\
            Only mark the goal as `complete` when ALL tasks are `verified` or `skipped`.\n",
            verified = verified,
            total = total,
            finished = finished,
            tasks = task_lines.join("\n"),
        )
    };

    format!(
        "\
# Active Thread Goal

<objective>
{objective}
</objective>

**Goal Status**: {status}
**Tokens Used**: {tokens_used} / {budget}
**Time Elapsed**: {time_seconds}s
{checklist_section}
## Instructions

Continue working toward the active thread goal above. You are in an auto-continuation
turn — the user expects you to make progress without additional prompting.

### Completion Audit Rules
Before marking this goal as complete:
1. A checklist MUST exist and ALL tasks must be `verified` or `skipped`.
2. All tests must pass and build must succeed.
3. The final deliverable must be ready for end-user use.
4. Do not call update_goal(complete) unless all verification steps pass.
5. If you try to call update_goal(complete) with unfinished tasks, it will be REJECTED.

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
        checklist_section = checklist_section,
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
