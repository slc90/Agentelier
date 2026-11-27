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

pub(super) async fn validate_session_files(
    elements: &[ComposerElement],
    conversation: &agentelier_conversation::Conversation,
    store: &agentelier_session_store::SessionStore,
) -> Result<(), ConversationFailure> {
    if !contains_file(elements) {
        return Ok(());
    }
    let view = conversation
        .current()
        .ok_or_else(|| ConversationFailure::new("文件引用缺少会话目录"))?;
    let project = store.session_project(view.id).await.map_err(|failure| {
        ConversationFailure::new(format!("无法核对文件引用的项目归属：{failure}"))
    })?;
    validate_files(elements, view.settings.working_directory, project, store).await
}

fn contains_file(elements: &[ComposerElement]) -> bool {
    elements
        .iter()
        .any(|element| matches!(element.content, ComposerContent::File { .. }))
}

pub(super) async fn validate_files(
    elements: &[ComposerElement],
    working_directory: std::path::PathBuf,
    project: Option<agentelier_conversation::ProjectId>,
    store: &agentelier_session_store::SessionStore,
) -> Result<(), ConversationFailure> {
    let files = elements
        .iter()
        .filter_map(|element| match &element.content {
            ComposerContent::File { reference } => Some(reference.clone()),
            _ => None,
        })
        .collect::<Vec<_>>();
    if files.is_empty() {
        return Ok(());
    }
    let project = match project {
        Some(project) => Some(store.project_directory(project).await.map_err(|failure| {
            ConversationFailure::new(format!("无法读取文件引用的项目目录：{failure}"))
        })?),
        None => None,
    };
    let scope = agentelier_conversation::FileScope {
        working_directory,
        project,
    };
    tokio::task::spawn_blocking(move || {
        files
            .iter()
            .try_for_each(|file| file.verify(&scope).map(drop))
    })
    .await
    .map_err(|failure| ConversationFailure::new(failure.to_string()))?
    .map_err(|failure| ConversationFailure::new(failure.to_string()))
}
