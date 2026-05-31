use agent_client_protocol::schema::{
    PermissionOption, PermissionOptionKind, RequestPermissionOutcome, RequestPermissionRequest,
    ToolCallStatus, ToolCallUpdate, ToolCallUpdateFields,
};
use agent_client_protocol::{Client, ConnectionTo};
use anyhow::Result;
use navi_sdk::{ApprovalDecision, ApprovalRequest};

use crate::acp::schema::acp_error_to_anyhow;

pub(crate) async fn request_permission(
    connection: &ConnectionTo<Client>,
    session_id: &str,
    request: &ApprovalRequest,
) -> Result<ApprovalDecision> {
    let tool_call = ToolCallUpdate::new(
        request.id.clone(),
        ToolCallUpdateFields::new()
            .title(request.summary.clone())
            .status(ToolCallStatus::Pending),
    );
    let response = connection
        .send_request(RequestPermissionRequest::new(
            session_id.to_string(),
            tool_call,
            vec![
                PermissionOption::new("allow_once", "Allow once", PermissionOptionKind::AllowOnce),
                PermissionOption::new("deny_once", "Deny", PermissionOptionKind::RejectOnce),
            ],
        ))
        .block_task()
        .await
        .map_err(acp_error_to_anyhow)?;

    match response.outcome {
        RequestPermissionOutcome::Selected(selected)
            if selected.option_id.0.as_ref() == "allow_once" =>
        {
            Ok(ApprovalDecision::Approved {
                id: request.id.clone(),
            })
        }
        _ => Ok(ApprovalDecision::Denied {
            id: request.id.clone(),
        }),
    }
}
