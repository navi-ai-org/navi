#[cfg(test)]
mod tests {
    use crate::goal::types::{GoalId, GoalStatus, SessionGoal};
    use crate::goal::{GoalExtension, GoalRuntimeHandle, GoalService};
    use crate::tool::{Tool, ToolInvocation};
    use serde_json::json;
    use std::sync::Arc;

    // ── types ──────────────────────────────────────────────────

    #[test]
    fn goal_id_is_unique() {
        let a = GoalId::new();
        let b = GoalId::new();
        assert_ne!(a, b);
    }

    #[test]
    fn goal_id_roundtrip_string() {
        let id = GoalId::new();
        let s = id.to_string();
        let id2 = GoalId::from_string(&s);
        assert_eq!(id, id2);
    }

    #[test]
    fn goal_status_terminal() {
        assert!(GoalStatus::Complete.is_terminal());
        assert!(GoalStatus::BudgetLimited.is_terminal());
        assert!(!GoalStatus::Active.is_terminal());
        assert!(!GoalStatus::Paused.is_terminal());
        assert!(!GoalStatus::Blocked.is_terminal());
        assert!(!GoalStatus::UsageLimited.is_terminal());
    }

    #[test]
    fn goal_status_auto_continue() {
        assert!(GoalStatus::Active.should_auto_continue());
        assert!(!GoalStatus::UsageLimited.should_auto_continue());
        assert!(!GoalStatus::Complete.should_auto_continue());
        assert!(!GoalStatus::BudgetLimited.should_auto_continue());
        assert!(!GoalStatus::Paused.should_auto_continue());
        assert!(!GoalStatus::Blocked.should_auto_continue());
    }

    #[test]
    fn session_goal_new_is_active() {
        let goal = SessionGoal::new("s1".into(), "test objective".into(), None);
        assert_eq!(goal.status, GoalStatus::Active);
        assert_eq!(goal.tokens_used, 0);
        assert_eq!(goal.time_used_seconds, 0);
    }

    #[test]
    fn session_goal_record_tokens() {
        let mut goal = SessionGoal::new("s1".into(), "test".into(), Some(100));
        assert!(!goal.record_tokens(50));
        assert_eq!(goal.tokens_used, 50);
        assert_eq!(goal.status, GoalStatus::Active);
        assert!(goal.record_tokens(60)); // 50+60=110 > 100 budget
        assert_eq!(goal.tokens_used, 110);
        assert_eq!(goal.status, GoalStatus::BudgetLimited);
    }

    #[test]
    fn session_goal_no_budget_never_exceeded() {
        let mut goal = SessionGoal::new("s1".into(), "test".into(), None);
        assert!(!goal.record_tokens(1_000_000));
        assert_eq!(goal.status, GoalStatus::Active);
    }

    #[test]
    fn session_goal_blocked_after_three_same_reason() {
        let mut goal = SessionGoal::new("s1".into(), "test".into(), None);
        assert!(!goal.record_blocked_turn("api down"));
        assert_eq!(goal.consecutive_blocked_turns, 1);
        assert!(!goal.record_blocked_turn("api down"));
        assert_eq!(goal.consecutive_blocked_turns, 2);
        assert!(goal.record_blocked_turn("api down"));
        assert_eq!(goal.status, GoalStatus::Blocked);
        assert_eq!(goal.consecutive_blocked_turns, 3);
    }

    #[test]
    fn session_goal_different_reasons_resets_counter() {
        let mut goal = SessionGoal::new("s1".into(), "test".into(), None);
        goal.record_blocked_turn("api down");
        goal.record_blocked_turn("api down");
        assert_eq!(goal.consecutive_blocked_turns, 2);
        assert!(!goal.record_blocked_turn("network error"));
        assert_eq!(goal.consecutive_blocked_turns, 1); // reset
    }

    #[test]
    fn session_goal_remaining_budget() {
        let goal = SessionGoal::new("s1".into(), "test".into(), Some(200));
        assert_eq!(goal.remaining_budget(), Some(200));
    }

    #[test]
    fn session_goal_transition_to() {
        let mut goal = SessionGoal::new("s1".into(), "test".into(), None);
        goal.transition_to(GoalStatus::Paused);
        assert_eq!(goal.status, GoalStatus::Paused);
    }

    // ── runtime ────────────────────────────────────────────────

    #[test]
    fn runtime_get_goal_none_by_default() {
        let runtime = GoalRuntimeHandle::new(None);
        assert!(runtime.get_goal().is_none());
    }

    #[test]
    fn runtime_set_and_get_objective() {
        let runtime = GoalRuntimeHandle::new(None);
        runtime.set_session_id("sess-1");
        let goal = runtime.set_objective("new objective".into(), Some(500));
        assert_eq!(goal.objective, "new objective");
        assert_eq!(goal.session_id, "sess-1");
        assert_eq!(goal.token_budget, Some(500));
        assert_eq!(goal.status, GoalStatus::Active);

        let got = runtime.get_goal().unwrap();
        assert_eq!(got.objective, "new objective");
    }

    #[test]
    fn runtime_clear_goal() {
        let runtime = GoalRuntimeHandle::new(None);
        runtime.set_objective("test".into(), None);
        assert!(runtime.get_goal().is_some());
        runtime.clear_goal();
        assert!(runtime.get_goal().is_none());
    }

    #[test]
    fn runtime_continue_if_idle_returns_prompt() {
        let runtime = GoalRuntimeHandle::new(None);
        runtime.set_objective("do work".into(), None);
        let prompt = runtime.continue_if_idle();
        assert!(prompt.is_some());
        assert!(prompt.unwrap().contains("do work"));
    }

    #[test]
    fn runtime_continue_if_idle_returns_none_when_empty() {
        let runtime = GoalRuntimeHandle::new(None);
        assert!(runtime.continue_if_idle().is_none());
    }

    #[test]
    fn runtime_continue_if_idle_disabled() {
        let runtime = GoalRuntimeHandle::new(None);
        runtime.set_objective("do work".into(), None);
        runtime.set_auto_continue(false);
        assert!(runtime.continue_if_idle().is_none());
    }

    #[test]
    fn runtime_record_tokens_and_budget_limit() {
        let runtime = GoalRuntimeHandle::new(None);
        runtime.set_objective("test".into(), Some(100));
        runtime.start_turn();
        assert!(!runtime.record_tokens(80));
        assert!(runtime.record_tokens(30)); // exceeds 100
        let prompt = runtime.budget_limit_prompt();
        assert!(prompt.is_some());
        assert!(prompt.unwrap().contains("Budget Limit Reached"));
    }

    #[test]
    fn runtime_record_blocked_turn() {
        let runtime = GoalRuntimeHandle::new(None);
        runtime.set_objective("test".into(), None);
        assert!(!runtime.record_blocked_turn("a"));
        assert!(!runtime.record_blocked_turn("a"));
        assert!(runtime.record_blocked_turn("a")); // 3rd
        let goal = runtime.get_goal().unwrap();
        assert_eq!(goal.status, GoalStatus::Blocked);
    }

    #[tokio::test]
    async fn runtime_status_update_survives_turn_finish() {
        let runtime = Arc::new(GoalRuntimeHandle::new(None));
        runtime.set_session_id("sess-1");
        runtime.set_objective("test".into(), None);
        runtime.start_turn();

        let tool = crate::goal::UpdateGoalTool::new(runtime.clone());
        let result = tool
            .invoke(ToolInvocation {
                id: "goal-update-1".to_string(),
                tool_name: "update_goal".to_string(),
                input: json!({ "action": "complete" }),
            })
            .await
            .expect("update goal");
        // Without a checklist, complete should be rejected.
        assert!(!result.ok);

        runtime.finish_turn();
        let goal = runtime.get_goal().unwrap();
        assert_eq!(goal.status, GoalStatus::Active);
    }

    #[tokio::test]
    async fn update_goal_blocked_preserves_consecutive_count() {
        let runtime = Arc::new(GoalRuntimeHandle::new(None));
        runtime.set_session_id("sess-1");
        runtime.set_objective("test".into(), None);
        let tool = crate::goal::UpdateGoalTool::new(runtime.clone());

        for expected_count in 1..=3 {
            let result = tool
                .invoke(ToolInvocation {
                    id: format!("goal-blocked-{expected_count}"),
                    tool_name: "update_goal".to_string(),
                    input: json!({
                        "action": "blocked",
                        "reason": "same blocker"
                    }),
                })
                .await
                .expect("update goal");
            assert!(result.ok);
            assert_eq!(
                result.output["consecutive_blocked_turns"].as_u64(),
                Some(expected_count)
            );
        }

        let goal = runtime.get_goal().unwrap();
        assert_eq!(goal.status, GoalStatus::Blocked);
        assert_eq!(goal.consecutive_blocked_turns, 3);
        assert_eq!(goal.block_reason.as_deref(), Some("same blocker"));
    }

    #[tokio::test]
    async fn update_goal_cannot_resume_terminal_goal() {
        let runtime = Arc::new(GoalRuntimeHandle::new(None));
        runtime.set_session_id("sess-1");
        let mut goal = runtime.set_objective("test".into(), Some(10));
        goal.transition_to(GoalStatus::BudgetLimited);
        runtime.update_goal(goal);

        let tool = crate::goal::UpdateGoalTool::new(runtime.clone());
        let result = tool
            .invoke(ToolInvocation {
                id: "goal-resume-terminal".to_string(),
                tool_name: "update_goal".to_string(),
                input: json!({ "action": "resume" }),
            })
            .await
            .expect("update goal");

        assert!(!result.ok);
        assert_eq!(
            runtime.get_goal().unwrap().status,
            GoalStatus::BudgetLimited
        );
    }

    #[test]
    fn runtime_mark_usage_limited() {
        let runtime = GoalRuntimeHandle::new(None);
        runtime.set_objective("test".into(), None);
        runtime.start_turn();
        runtime.mark_usage_limited();
        let goal = runtime.get_goal().unwrap();
        assert_eq!(goal.status, GoalStatus::UsageLimited);
    }

    // ── accounting ─────────────────────────────────────────────

    #[test]
    fn accounting_basic_flow() {
        let goal = SessionGoal::new("s1".into(), "test".into(), Some(200));
        let acct = crate::goal::GoalAccountingState::new(goal);
        assert!(acct.is_active());

        acct.start_turn();
        assert!(!acct.record_token_usage(100));
        assert!(acct.record_token_usage(150)); // exceeds 200

        let snap = acct.snapshot().unwrap();
        assert_eq!(snap.status, GoalStatus::BudgetLimited);
        assert!(snap.tokens_used >= 200);
    }

    // ── service ────────────────────────────────────────────────

    #[test]
    fn service_set_get_clear_goal() {
        let svc = GoalService::new();
        let goal = svc.set_goal("s1".into(), "obj".into(), Some(500));
        assert_eq!(goal.objective, "obj");

        let got = svc.get_goal("s1");
        assert!(got.is_none()); // runtime not registered

        svc.clear_goal("s1");
    }

    #[test]
    fn service_register_and_use_runtime() {
        let svc = GoalService::new();
        let rt = Arc::new(GoalRuntimeHandle::new(None));

        svc.register_runtime("s1".into(), rt.clone());
        let goal = svc.set_goal("s1".into(), "through_runtime".into(), None);
        assert_eq!(goal.objective, "through_runtime");

        let got = svc.get_goal("s1").unwrap();
        assert_eq!(got.objective, "through_runtime");

        svc.clear_goal("s1");
        assert!(svc.get_goal("s1").is_none());
    }

    // ── steering ───────────────────────────────────────────────

    #[test]
    fn steering_continuation_has_objective() {
        let goal = SessionGoal::new("x".into(), "build feature Y".into(), Some(1000));
        let prompt = crate::goal::steering::build_continuation_prompt(&goal);
        assert!(prompt.contains("build feature Y"));
        assert!(prompt.contains("Active Thread Goal"));
        assert!(prompt.contains("Completion Audit"));
        assert!(prompt.contains("Blocked Audit"));
    }

    #[test]
    fn steering_budget_limit() {
        let mut goal = SessionGoal::new("x".into(), "task".into(), Some(10));
        goal.record_tokens(15);
        let prompt = crate::goal::steering::build_budget_limit_prompt(&goal);
        assert!(prompt.contains("Budget Limit Reached"));
        assert!(prompt.contains("task"));
    }

    #[test]
    fn steering_objective_updated() {
        let goal = SessionGoal::new("x".into(), "new task".into(), None);
        let prompt = crate::goal::steering::build_objective_updated_prompt(&goal);
        assert!(prompt.contains("Objective Updated"));
        assert!(prompt.contains("new task"));
    }

    // ── tools definitions ──────────────────────────────────────

    #[test]
    fn goal_tool_definitions_exist() {
        let defs = crate::goal::goal_tool_definitions();
        assert_eq!(defs.len(), 4);
        let names: Vec<&str> = defs.iter().map(|d| d.name.as_str()).collect();
        assert!(names.contains(&"get_goal"));
        assert!(names.contains(&"create_goal"));
        assert!(names.contains(&"update_goal"));
        assert!(names.contains(&"update_goal_checklist"));
    }

    #[test]
    fn get_goal_tool_returns_none_when_no_goal() {
        let rt = Arc::new(GoalRuntimeHandle::new(None));
        let tool = crate::goal::GetGoalTool::new(rt);
        let def = tool.definition();
        assert_eq!(def.name, "get_goal");
    }

    #[test]
    fn create_goal_tool_definition() {
        let rt = Arc::new(GoalRuntimeHandle::new(None));
        let tool = crate::goal::CreateGoalTool::new(rt);
        let def = tool.definition();
        assert_eq!(def.name, "create_goal");
        assert_eq!(def.kind, crate::tool::ToolKind::Write);
    }

    #[test]
    fn update_goal_tool_definition() {
        let rt = Arc::new(GoalRuntimeHandle::new(None));
        let tool = crate::goal::UpdateGoalTool::new(rt);
        let def = tool.definition();
        assert_eq!(def.name, "update_goal");
        assert_eq!(def.kind, crate::tool::ToolKind::Write);
    }

    // ── events ─────────────────────────────────────────────────

    #[test]
    fn runtime_event_goal_updated_converts_to_agent_event() {
        use crate::event::{AgentEvent, RuntimeEvent, RuntimeEventKind};
        let event = RuntimeEvent::new(RuntimeEventKind::GoalUpdated {
            session_id: "s1".into(),
            goal_id: "g1".into(),
            objective: "obj".into(),
            short_description: Some("short".into()),
            status: GoalStatus::Active,
            tokens_used: 42,
            token_budget: Some(100),
        });
        let agent = event.into_agent_event();
        assert!(matches!(agent, Some(AgentEvent::GoalUpdated { .. })));
    }

    #[test]
    fn session_snapshot_has_goal_field() {
        use crate::session::{SessionId, SessionSnapshot};
        let snap = SessionSnapshot {
            version: SessionSnapshot::CURRENT_VERSION,
            id: SessionId::new("test".into()),
            title: None,
            project: std::path::PathBuf::from("/tmp"),
            created_at: 1,
            updated_at: 2,
            events: vec![],
            memory: None,
            goal: Some(SessionGoal::new("s1".into(), "obj".into(), None)),
            usage: None,
        };
        assert!(snap.goal.is_some());
        let goal = snap.goal.as_ref().unwrap();
        assert_eq!(goal.objective, "obj");

        let json = serde_json::to_string(&snap).unwrap();
        let loaded: SessionSnapshot = serde_json::from_str(&json).unwrap();
        assert!(loaded.goal.is_some());
    }

    // ── extension ──────────────────────────────────────────────

    #[test]
    fn extension_hooks_basic_flow() {
        let svc = Arc::new(GoalService::new());
        let rt = Arc::new(GoalRuntimeHandle::new(None));
        let ext = GoalExtension::new(svc.clone(), rt.clone());

        ext.on_session_start("sess-1");
        rt.set_objective("work".into(), None);
        ext.on_turn_start("sess-1", "task");
        assert!(!ext.on_token_usage(50, 30));
        ext.on_turn_end("sess-1");
        ext.on_session_end("sess-1");
    }

    // ── Checklist tests ───────────────────────────────────────

    #[test]
    fn checklist_empty_is_not_complete() {
        let goal = SessionGoal::new("s1".into(), "obj".into(), None);
        assert!(!goal.is_checklist_complete());
    }

    #[test]
    fn checklist_all_verified_is_complete() {
        let mut goal = SessionGoal::new("s1".into(), "obj".into(), None);
        goal.checklist = vec![
            crate::goal::types::GoalTask {
                id: 0,
                description: "task 0".into(),
                status: crate::goal::types::TaskStatus::Verified,
                verification: Some("cargo test".into()),
                verified: true,
            },
            crate::goal::types::GoalTask {
                id: 1,
                description: "task 1".into(),
                status: crate::goal::types::TaskStatus::Verified,
                verification: Some("cargo build".into()),
                verified: true,
            },
        ];
        assert!(goal.is_checklist_complete());
        assert_eq!(goal.verified_count(), 2);
        assert_eq!(goal.finished_count(), 2);
    }

    #[test]
    fn checklist_with_pending_is_not_complete() {
        let mut goal = SessionGoal::new("s1".into(), "obj".into(), None);
        goal.checklist = vec![
            crate::goal::types::GoalTask {
                id: 0,
                description: "task 0".into(),
                status: crate::goal::types::TaskStatus::Verified,
                verification: None,
                verified: true,
            },
            crate::goal::types::GoalTask {
                id: 1,
                description: "task 1".into(),
                status: crate::goal::types::TaskStatus::Pending,
                verification: None,
                verified: false,
            },
        ];
        assert!(!goal.is_checklist_complete());
        assert_eq!(goal.finished_count(), 1);
        assert!(goal.next_unfinished_task().is_some());
    }

    #[test]
    fn checklist_skipped_counts_as_finished() {
        let mut goal = SessionGoal::new("s1".into(), "obj".into(), None);
        goal.checklist = vec![
            crate::goal::types::GoalTask {
                id: 0,
                description: "task 0".into(),
                status: crate::goal::types::TaskStatus::Verified,
                verification: None,
                verified: true,
            },
            crate::goal::types::GoalTask {
                id: 1,
                description: "task 1".into(),
                status: crate::goal::types::TaskStatus::Skipped,
                verification: None,
                verified: false,
            },
        ];
        assert!(goal.is_checklist_complete());
        assert_eq!(goal.verified_count(), 1);
        assert_eq!(goal.finished_count(), 2);
    }

    #[test]
    fn checklist_update_task_status() {
        let mut goal = SessionGoal::new("s1".into(), "obj".into(), None);
        goal.checklist = vec![
            crate::goal::types::GoalTask::new(0, "task 0".into()),
            crate::goal::types::GoalTask::new(1, "task 1".into()),
        ];
        assert!(goal.update_task_status(0, crate::goal::types::TaskStatus::InProgress));
        assert_eq!(
            goal.checklist[0].status,
            crate::goal::types::TaskStatus::InProgress
        );
        assert!(!goal.update_task_status(99, crate::goal::types::TaskStatus::Verified));
    }

    #[tokio::test]
    async fn complete_blocked_without_checklist() {
        let runtime = Arc::new(GoalRuntimeHandle::new(None));
        runtime.set_session_id("sess-1");
        runtime.set_objective("test".into(), None);

        let tool = crate::goal::UpdateGoalTool::new(runtime.clone());
        let result = tool
            .invoke(ToolInvocation {
                id: "t1".into(),
                tool_name: "update_goal".into(),
                input: json!({ "action": "complete" }),
            })
            .await
            .expect("invoke");
        assert!(!result.ok);
        assert!(
            result.output["error"]
                .as_str()
                .unwrap()
                .contains("checklist")
        );
    }

    #[tokio::test]
    async fn complete_blocked_with_unfinished_tasks() {
        let runtime = Arc::new(GoalRuntimeHandle::new(None));
        runtime.set_session_id("sess-1");
        runtime.set_objective("test".into(), None);

        // Set a checklist with one pending task
        runtime.update_checklist(vec![crate::goal::types::GoalTask::new(
            0,
            "implement feature".into(),
        )]);

        let tool = crate::goal::UpdateGoalTool::new(runtime.clone());
        let result = tool
            .invoke(ToolInvocation {
                id: "t1".into(),
                tool_name: "update_goal".into(),
                input: json!({ "action": "complete" }),
            })
            .await
            .expect("invoke");
        assert!(!result.ok);
        assert!(
            result.output["error"]
                .as_str()
                .unwrap()
                .contains("unfinished")
        );
    }

    #[tokio::test]
    async fn complete_allowed_with_all_verified() {
        let runtime = Arc::new(GoalRuntimeHandle::new(None));
        runtime.set_session_id("sess-1");
        runtime.set_objective("test".into(), None);

        // Set checklist and verify all tasks
        runtime.update_checklist(vec![
            crate::goal::types::GoalTask::new(0, "task 0".into()),
            crate::goal::types::GoalTask::new(1, "task 1".into()),
        ]);
        runtime.update_task_status(0, crate::goal::types::TaskStatus::Verified);
        runtime.update_task_status(1, crate::goal::types::TaskStatus::Verified);

        let tool = crate::goal::UpdateGoalTool::new(runtime.clone());
        let result = tool
            .invoke(ToolInvocation {
                id: "t1".into(),
                tool_name: "update_goal".into(),
                input: json!({ "action": "complete" }),
            })
            .await
            .expect("invoke");
        assert!(result.ok);
        assert_eq!(runtime.get_goal().unwrap().status, GoalStatus::Complete);
    }

    #[tokio::test]
    async fn checklist_set_via_tool() {
        let runtime = Arc::new(GoalRuntimeHandle::new(None));
        runtime.set_session_id("sess-1");
        runtime.set_objective("test".into(), None);

        let tool = crate::goal::UpdateGoalChecklistTool::new(runtime.clone());
        let result = tool
            .invoke(ToolInvocation {
                id: "t1".into(),
                tool_name: "update_goal_checklist".into(),
                input: json!({
                    "action": "set",
                    "tasks": ["write code", "run tests", "fix lint"]
                }),
            })
            .await
            .expect("invoke");
        assert!(result.ok);
        assert_eq!(result.output["task_count"].as_u64().unwrap(), 3);

        let goal = runtime.get_goal().unwrap();
        assert_eq!(goal.checklist.len(), 3);
        assert_eq!(goal.checklist[0].description, "write code");
        assert_eq!(
            goal.checklist[0].status,
            crate::goal::types::TaskStatus::Pending
        );
    }

    #[tokio::test]
    async fn checklist_update_status_via_tool() {
        let runtime = Arc::new(GoalRuntimeHandle::new(None));
        runtime.set_session_id("sess-1");
        runtime.set_objective("test".into(), None);
        runtime.update_checklist(vec![crate::goal::types::GoalTask::new(0, "task 0".into())]);

        let tool = crate::goal::UpdateGoalChecklistTool::new(runtime.clone());
        let result = tool
            .invoke(ToolInvocation {
                id: "t1".into(),
                tool_name: "update_goal_checklist".into(),
                input: json!({
                    "action": "update",
                    "task_id": 0,
                    "status": "verified",
                    "verification": "cargo test -p navi-core"
                }),
            })
            .await
            .expect("invoke");
        assert!(result.ok);
        assert_eq!(result.output["status"], "verified");

        let goal = runtime.get_goal().unwrap();
        assert_eq!(
            goal.checklist[0].status,
            crate::goal::types::TaskStatus::Verified
        );
        assert!(goal.checklist[0].verified);
        assert_eq!(
            goal.checklist[0].verification.as_deref(),
            Some("cargo test -p navi-core")
        );
    }

    #[test]
    fn steering_prompt_shows_checklist_warning_when_empty() {
        let goal = SessionGoal::new("s1".into(), "do stuff".into(), None);
        let prompt = crate::goal::steering::build_continuation_prompt(&goal);
        assert!(prompt.contains("NO CHECKLIST DEFINED"));
    }

    #[test]
    fn steering_prompt_shows_checklist_progress() {
        let mut goal = SessionGoal::new("s1".into(), "do stuff".into(), None);
        goal.checklist = vec![
            crate::goal::types::GoalTask {
                id: 0,
                description: "task A".into(),
                status: crate::goal::types::TaskStatus::Verified,
                verification: Some("cargo test".into()),
                verified: true,
            },
            crate::goal::types::GoalTask {
                id: 1,
                description: "task B".into(),
                status: crate::goal::types::TaskStatus::InProgress,
                verification: None,
                verified: false,
            },
        ];
        let prompt = crate::goal::steering::build_continuation_prompt(&goal);
        assert!(prompt.contains("1/2 verified"));
        assert!(prompt.contains("task A"));
        assert!(prompt.contains("task B"));
        assert!(prompt.contains("▶"));
        assert!(prompt.contains("✓"));
    }

    // ── End-to-end tool path (real tools, real runtime) ────────

    #[tokio::test]
    async fn create_goal_tool_sets_runtime_and_emits_goal_updated() {
        use crate::event::AgentEvent;
        use crate::tool::ToolInvocationContext;

        let rt = Arc::new(GoalRuntimeHandle::new(None));
        rt.set_session_id("sess-e2e");
        let tool = crate::goal::CreateGoalTool::new(rt.clone());
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();

        let result = tool
            .invoke_with_context(
                ToolInvocation {
                    id: "c1".into(),
                    tool_name: "create_goal".into(),
                    input: json!({
                        "objective": "ship goal loop",
                        "short_description": "goal loop",
                    }),
                },
                ToolInvocationContext {
                    event_tx: Some(tx),
                    ..Default::default()
                },
            )
            .await
            .expect("create_goal");

        assert!(result.ok, "{:?}", result.output);
        let goal = rt.get_goal().expect("goal set");
        assert_eq!(goal.objective, "ship goal loop");
        assert_eq!(goal.status, GoalStatus::Active);
        assert!(rt.continue_if_idle().is_some());

        let event = rx.try_recv().expect("GoalUpdated event");
        assert!(matches!(
            event,
            AgentEvent::GoalUpdated {
                status: GoalStatus::Active,
                ..
            }
        ));
    }

    #[tokio::test]
    async fn goal_tool_path_set_progress_complete_clear() {
        use crate::tool::ToolInvocationContext;

        let rt = Arc::new(GoalRuntimeHandle::new(None));
        rt.set_session_id("sess-lifecycle");
        let create = crate::goal::CreateGoalTool::new(rt.clone());
        let checklist = crate::goal::UpdateGoalChecklistTool::new(rt.clone());
        let update = crate::goal::UpdateGoalTool::new(rt.clone());
        let get = crate::goal::GetGoalTool::new(rt.clone());
        let ctx = ToolInvocationContext::default();

        // set
        let r = create
            .invoke_with_context(
                ToolInvocation {
                    id: "1".into(),
                    tool_name: "create_goal".into(),
                    input: json!({"objective": "verify lifecycle"}),
                },
                ctx.clone(),
            )
            .await
            .unwrap();
        assert!(r.ok);

        // checklist
        let r = checklist
            .invoke_with_context(
                ToolInvocation {
                    id: "2".into(),
                    tool_name: "update_goal_checklist".into(),
                    input: json!({
                        "action": "set",
                        "tasks": ["step one", "step two"]
                    }),
                },
                ctx.clone(),
            )
            .await
            .unwrap();
        assert!(r.ok);

        // verify both tasks
        for id in [0usize, 1usize] {
            let r = checklist
                .invoke_with_context(
                    ToolInvocation {
                        id: format!("v{id}"),
                        tool_name: "update_goal_checklist".into(),
                        input: json!({
                            "action": "update",
                            "task_id": id,
                            "status": "verified",
                            "verification": "unit test"
                        }),
                    },
                    ctx.clone(),
                )
                .await
                .unwrap();
            assert!(r.ok, "{:?}", r.output);
        }

        // complete
        let r = update
            .invoke_with_context(
                ToolInvocation {
                    id: "done".into(),
                    tool_name: "update_goal".into(),
                    input: json!({"action": "complete"}),
                },
                ctx.clone(),
            )
            .await
            .unwrap();
        assert!(r.ok, "{:?}", r.output);
        assert_eq!(rt.get_goal().unwrap().status, GoalStatus::Complete);
        // terminal → no auto-continue
        assert!(rt.continue_if_idle().is_none());

        // get still returns terminal goal
        let r = get
            .invoke(ToolInvocation {
                id: "g".into(),
                tool_name: "get_goal".into(),
                input: json!({}),
            })
            .await
            .unwrap();
        assert!(r.ok);
        assert_eq!(r.output["status"], "complete");

        // clear
        rt.clear_goal();
        assert!(rt.get_goal().is_none());
        assert!(rt.continue_if_idle().is_none());
    }

    #[tokio::test]
    async fn update_goal_pause_and_resume_toggle_auto_continue() {
        use crate::tool::ToolInvocationContext;

        let rt = Arc::new(GoalRuntimeHandle::new(None));
        rt.set_session_id("sess-pause");
        rt.set_objective("work".into(), None);
        let update = crate::goal::UpdateGoalTool::new(rt.clone());
        let ctx = ToolInvocationContext::default();

        assert!(rt.continue_if_idle().is_some());

        let r = update
            .invoke_with_context(
                ToolInvocation {
                    id: "p".into(),
                    tool_name: "update_goal".into(),
                    input: json!({"action": "pause"}),
                },
                ctx.clone(),
            )
            .await
            .unwrap();
        assert!(r.ok);
        assert_eq!(rt.get_goal().unwrap().status, GoalStatus::Paused);
        assert!(rt.continue_if_idle().is_none());

        let r = update
            .invoke_with_context(
                ToolInvocation {
                    id: "r".into(),
                    tool_name: "update_goal".into(),
                    input: json!({"action": "resume"}),
                },
                ctx,
            )
            .await
            .unwrap();
        assert!(r.ok);
        assert_eq!(rt.get_goal().unwrap().status, GoalStatus::Active);
        assert!(rt.continue_if_idle().is_some());
    }

    #[test]
    fn goal_tools_are_direct_exposure() {
        let defs = crate::goal::goal_tool_definitions();
        for def in defs {
            assert_eq!(
                def.metadata.exposure,
                crate::tool::ToolExposure::Direct,
                "{} should be Direct so the model always sees it",
                def.name
            );
        }
    }
}
