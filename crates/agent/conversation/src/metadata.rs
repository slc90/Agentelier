use crate::ConversationFailure;

pub fn session_title(value: &str) -> Result<String, ConversationFailure> {
    let title = value
        .chars()
        .map(|character| {
            if character.is_control() {
                ' '
            } else {
                character
            }
        })
        .collect::<String>();
    let title = title.trim();
    if title.is_empty() {
        return Err(ConversationFailure::new("会话名称不能为空"));
    }
    Ok(title.into())
}
