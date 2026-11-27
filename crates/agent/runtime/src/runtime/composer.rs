use agentelier_conversation::{
    ComposerContent, ComposerElement, ConversationFailure, McpCapability, bound_skills,
    validate_mcp_input,
};
use agentelier_mcp::{ConnectionState, Connections};
use agentelier_skills::UserSkills;
use tokio_util::sync::CancellationToken;

pub(super) async fn validate(
    elements: &[ComposerElement],
    connections: &Connections,
    skills: &UserSkills,
) -> Result<(), ConversationFailure> {
    if elements
        .iter()
        .any(|element| matches!(element.content, ComposerContent::Mcp { .. }))
    {
        let available = connections
            .statuses()
            .into_iter()
            .filter(|status| matches!(status.state, ConnectionState::Ready(_)))
            .flat_map(|status| status.tool_definitions)
            .filter_map(|definition| McpCapability::from_definition(&definition))
            .collect::<Vec<_>>();
        validate_mcp_input(elements, &available)?;
    }
    let bindings = bound_skills(elements);
    if !bindings.is_empty() {
        let root = skills.root_path().to_owned();
        tokio::task::spawn_blocking(move || {
            let cancel = CancellationToken::new();
            let catalog = agentelier_skills::discover_standard(&root, true, &cancel)?;
            agentelier_skills::select(&catalog, "", &bindings, &cancel).map(drop)
        })
        .await
        .map_err(|failure| ConversationFailure::new(failure.to_string()))?
        .map_err(|failure| ConversationFailure::new(failure.to_string()))?;
    }
    Ok(())
}
