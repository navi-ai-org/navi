//! plan review: line selection, comments, approve/changes/quit.

use navi_core::{Plan, PlanLineComment, PlanStatus, PlanStore, plan_view_lines};

use crate::TuiApp;
use crate::keybindings::{close_active_modal, replace_modal};
use crate::notifications::show_notification;
use crate::state::ModalKind;

/// Which pane of the plan review modal has focus.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum PlanReviewFocus {
    #[default]
    Preview,
    CommentInput,
    Prompt,
}

/// Full state for the plan review modal.
#[derive(Debug, Clone)]
pub struct PlanReviewState {
    pub plan_id: String,
    /// Tool invocation id — used to unblock the agent turn.
    pub invocation_id: String,
    pub title: String,
    pub lines: Vec<String>,
    pub comments: Vec<PlanLineComment>,
    pub scroll: usize,
    pub cursor_line: usize,
    /// Anchor for range selection (inclusive with cursor_line).
    pub sel_anchor: Option<usize>,
    pub focus: PlanReviewFocus,
    pub comment_draft: String,
    pub comment_cursor: usize,
    pub prompt_draft: String,
    pub prompt_cursor: usize,
    #[allow(dead_code)]
    pub project_id: String,
}

impl PlanReviewState {
    pub fn from_plan(plan: &Plan, invocation_id: String) -> Self {
        let lines = plan_view_lines(plan);
        Self {
            plan_id: plan.id.clone(),
            invocation_id,
            title: plan.title.clone(),
            lines,
            comments: plan.comments.clone(),
            scroll: 0,
            cursor_line: 0,
            sel_anchor: None,
            focus: PlanReviewFocus::Preview,
            comment_draft: String::new(),
            comment_cursor: 0,
            prompt_draft: String::new(),
            prompt_cursor: 0,
            project_id: plan.project_id.clone(),
        }
    }

    pub fn selected_range(&self) -> (usize, usize) {
        let cur = self.cursor_line.min(self.lines.len().saturating_sub(1));
        match self.sel_anchor {
            Some(a) => {
                let a = a.min(self.lines.len().saturating_sub(1));
                (a.min(cur), a.max(cur))
            }
            None => (cur, cur),
        }
    }

    pub fn comment_on_line(&self, line: usize) -> Option<&PlanLineComment> {
        self.comments
            .iter()
            .find(|c| line >= c.start_line && line <= c.end_line)
    }

    pub fn clamp_cursor(&mut self) {
        if self.lines.is_empty() {
            self.cursor_line = 0;
            return;
        }
        self.cursor_line = self.cursor_line.min(self.lines.len() - 1);
        if let Some(a) = self.sel_anchor {
            self.sel_anchor = Some(a.min(self.lines.len() - 1));
        }
    }

    pub fn ensure_cursor_visible(&mut self, visible: usize) {
        let visible = visible.max(1);
        if self.cursor_line < self.scroll {
            self.scroll = self.cursor_line;
        } else if self.cursor_line >= self.scroll + visible {
            self.scroll = self.cursor_line + 1 - visible;
        }
    }
}

/// Open plan review modal from a stored plan (blocking tool wait).
pub(crate) fn open_plan_review(app: &mut TuiApp, plan: Plan, invocation_id: String) {
    app.plan_review = Some(PlanReviewState::from_plan(&plan, invocation_id));
    app.proposed_plan = Some(navi_sdk::ProposedPlan {
        title: plan.title.clone(),
        steps: plan.steps.iter().map(|s| s.description.clone()).collect(),
    });
    // Seed the progress strip immediately (status=proposed until approved).
    app.active_plan = Some(active_plan_from_store_plan(&plan, "proposed"));
    replace_modal(app, ModalKind::ConfirmPlan);
    show_notification(app, "Plan Review", "Waiting for your review…");
}

pub(crate) fn active_plan_from_store_plan(
    plan: &Plan,
    status: &str,
) -> crate::state::ActivePlanUiState {
    crate::state::ActivePlanUiState {
        plan_id: plan.id.clone(),
        title: plan.title.clone(),
        steps: plan
            .steps
            .iter()
            .map(|s| crate::state::ActivePlanStepUi {
                description: s.description.clone(),
                completed: s.completed,
            })
            .collect(),
        status: status.to_string(),
        expanded: false,
    }
}

/// Open review from PlanProposed event (tag path — non-blocking legacy).
pub(crate) fn open_plan_review_from_proposed(app: &mut TuiApp, title: String, steps: Vec<String>) {
    let plan = Plan {
        id: format!("proposed-{}", navi_core::plan_store::now_ms()),
        title: title.clone(),
        description: String::new(),
        steps: steps
            .iter()
            .map(|d| navi_core::PlanStep {
                description: d.clone(),
                completed: false,
                notes: String::new(),
            })
            .collect(),
        status: PlanStatus::Proposed,
        created_at: navi_core::plan_store::now_ms(),
        updated_at: navi_core::plan_store::now_ms(),
        body_markdown: String::new(),
        comments: Vec::new(),
        project_id: String::new(),
        session_id: app.session_id.as_str().to_string(),
    };
    if let Ok(store) = plan_store_for_app(app) {
        let _ = store.upsert(&plan);
    }
    // No invocation to unblock for pure text-tag proposals.
    open_plan_review(app, plan, String::new());
}

/// Open from blocking PlanReviewRequested event.
pub(crate) fn open_plan_review_from_request(
    app: &mut TuiApp,
    request: navi_sdk::PlanReviewRequest,
) {
    if let Ok(store) = plan_store_for_app(app)
        && let Ok(Some(plan)) = store.get(&request.plan_id)
    {
        open_plan_review(app, plan, request.id);
        return;
    }
    let plan = Plan {
        id: request.plan_id.clone(),
        title: request.title.clone(),
        description: request.description.clone(),
        steps: request
            .steps
            .iter()
            .map(|d| navi_core::PlanStep {
                description: d.clone(),
                completed: false,
                notes: String::new(),
            })
            .collect(),
        status: PlanStatus::Proposed,
        created_at: navi_core::plan_store::now_ms(),
        updated_at: navi_core::plan_store::now_ms(),
        body_markdown: String::new(),
        comments: Vec::new(),
        project_id: String::new(),
        session_id: app.session_id.as_str().to_string(),
    };
    open_plan_review(app, plan, request.id);
}

pub(crate) fn plan_store_for_app(app: &TuiApp) -> anyhow::Result<PlanStore> {
    // Same data_dir the PlanTool uses (LoadedConfig / project dirs).
    let data_dir = app.loaded_config.data_dir.clone();
    PlanStore::open_default(&data_dir)
}

/// Start commenting on the current selection.
pub(crate) fn begin_comment(app: &mut TuiApp) {
    let Some(review) = app.plan_review.as_mut() else {
        return;
    };
    review.focus = PlanReviewFocus::CommentInput;
    review.comment_draft.clear();
    review.comment_cursor = 0;
}

/// Commit comment draft onto the selected range.
pub(crate) fn commit_comment(app: &mut TuiApp) {
    let Some(review) = app.plan_review.as_mut() else {
        return;
    };
    let text = review.comment_draft.trim().to_string();
    if text.is_empty() {
        review.focus = PlanReviewFocus::Preview;
        return;
    }
    let (start, end) = review.selected_range();
    review.comments.push(PlanLineComment {
        start_line: start,
        end_line: end,
        text,
    });
    review.comment_draft.clear();
    review.comment_cursor = 0;
    review.focus = PlanReviewFocus::Preview;
    review.sel_anchor = None;
    persist_comments(app);
}

fn persist_comments(app: &mut TuiApp) {
    let Some(review) = app.plan_review.as_ref() else {
        return;
    };
    if review.plan_id.starts_with("proposed-") || review.plan_id.is_empty() {
        // still try store
    }
    if let Ok(store) = plan_store_for_app(app) {
        let _ = store.save_comments(&review.plan_id, review.comments.clone());
    }
}

/// Approve plan (+ optional comments/prompt) and feed agent.
pub(crate) fn approve_plan(app: &mut TuiApp) {
    finish_review(app, "approve", PlanStatus::Active, true);
}

/// Request changes with comments/prompt.
pub(crate) fn request_plan_changes(app: &mut TuiApp) {
    finish_review(app, "request_changes", PlanStatus::Proposed, true);
}

/// Quit / abandon plan.
pub(crate) fn quit_plan(app: &mut TuiApp) {
    finish_review(app, "quit", PlanStatus::Abandoned, false);
}

fn finish_review(app: &mut TuiApp, decision: &str, next_status: PlanStatus, _send_feedback: bool) {
    let Some(review) = app.plan_review.take() else {
        close_active_modal(app);
        return;
    };

    let store = plan_store_for_app(app).ok();
    if let Some(store) = &store
        && let Ok(Some(mut plan)) = store.get(&review.plan_id)
    {
        plan.status = next_status;
        plan.comments = review.comments.clone();
        plan.updated_at = navi_core::plan_store::now_ms();
        let _ = store.upsert(&plan);
    }

    let decision_enum = match decision {
        "approve" => navi_sdk::PlanReviewDecision::Approve,
        "request_changes" => navi_sdk::PlanReviewDecision::RequestChanges,
        _ => navi_sdk::PlanReviewDecision::Quit,
    };

    // Unblock the agent turn — model only continues after this resolves.
    if !review.invocation_id.is_empty() {
        let session_id = app.session_id.as_str().to_string();
        let engine = app.engine();
        let response = navi_sdk::PlanReviewResponse {
            id: review.invocation_id.clone(),
            plan_id: review.plan_id.clone(),
            decision: decision_enum,
            comments: review.comments.clone(),
            freeform: review.prompt_draft.clone(),
        };
        tokio::spawn(async move {
            let _ = engine.resolve_plan_review(&session_id, response).await;
        });
    }

    // Exit plan mode on approve/quit when applicable.
    if decision == "approve" || decision == "quit" {
        let session_id = app.session_id.as_str().to_string();
        let engine = app.engine();
        tokio::spawn(async move {
            let _ = engine.exit_plan_mode(&session_id).await;
        });
    }

    // Update the live plan progress strip.
    match decision {
        "approve" => {
            if let Some(plan) = app.active_plan.as_mut() {
                plan.status = "active".into();
                plan.expanded = false;
            } else if let Some(proposed) = app.proposed_plan.as_ref() {
                app.active_plan = Some(crate::state::ActivePlanUiState {
                    plan_id: review.plan_id.clone(),
                    title: review.title.clone(),
                    steps: proposed
                        .steps
                        .iter()
                        .map(|d| crate::state::ActivePlanStepUi {
                            description: d.clone(),
                            completed: false,
                        })
                        .collect(),
                    status: "active".into(),
                    expanded: false,
                });
            }
        }
        "quit" => {
            app.active_plan = None;
        }
        _ => {}
    }

    show_notification(
        app,
        "Plan Review",
        match decision {
            "approve" => "Approved — agent continues.",
            "request_changes" => "Changes requested — agent revises.",
            "quit" => "Plan abandoned.",
            _ => "Closed.",
        },
    );

    app.proposed_plan = None;
    close_active_modal(app);
}
