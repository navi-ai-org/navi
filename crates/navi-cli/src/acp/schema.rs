use agent_client_protocol::schema::{
    ContentBlock, ToolCallContent, ToolCallStatus, ToolCallUpdate, ToolCallUpdateFields, ToolKind,
};
use navi_sdk::ToolResult;

pub(crate) fn tool_result_update(result: &ToolResult) -> ToolCallUpdate {
    let content = ToolCallContent::from(ContentBlock::Text(
        agent_client_protocol::schema::TextContent::new(tool_output_text(&result.output)),
    ));
    ToolCallUpdate::new(
        result.invocation_id.clone(),
        ToolCallUpdateFields::new()
            .status(if result.ok {
                ToolCallStatus::Completed
            } else {
                ToolCallStatus::Failed
            })
            .content(vec![content])
            .raw_output(serde_json::json!({
                "ok": result.ok,
                "output": result.output,
            })),
    )
}

pub(crate) fn tool_output_text(output: &serde_json::Value) -> String {
    match output {
        serde_json::Value::String(text) => text.clone(),
        other => serde_json::to_string_pretty(other).unwrap_or_else(|_| other.to_string()),
    }
}

pub(crate) fn acp_tool_kind(tool_name: &str) -> ToolKind {
    match tool_name {
        "read_file" | "fs_browser" => ToolKind::Read,
        "write_file" | "apply_patch" => ToolKind::Edit,
        "grep" => ToolKind::Search,
        "bash" | "git_ops" => ToolKind::Execute,
        "package_manager" => ToolKind::Edit,
        _ => ToolKind::Other,
    }
}

pub(crate) fn prompt_to_text(blocks: Vec<ContentBlock>) -> String {
    blocks
        .into_iter()
        .filter_map(|block| match block {
            ContentBlock::Text(text) => Some(text.text),
            ContentBlock::ResourceLink(link) => Some(format!("Resource: {}", link.uri)),
            ContentBlock::Resource(resource) => Some(format!("Resource: {:?}", resource.resource)),
            _ => None,
        })
        .collect::<Vec<_>>()
        .join("\n\n")
}

pub(crate) fn acp_error_to_anyhow(error: agent_client_protocol::Error) -> anyhow::Error {
    anyhow::anyhow!("{error:?}")
}
