use super::{MAX_ACTIONS, invalid};
use crate::ToolFailure;

pub(super) enum Action {
    Add {
        path: String,
        content: String,
    },
    Delete {
        path: String,
    },
    Update {
        path: String,
        target: Option<String>,
        chunks: Vec<Chunk>,
    },
}
pub(super) struct Chunk {
    pub(super) anchor: Option<String>,
    pub(super) old: Vec<String>,
    pub(super) new: Vec<String>,
    pub(super) eof: bool,
}

pub(super) fn parse(text: &str) -> Result<Vec<Action>, ToolFailure> {
    let lines: Vec<_> = text.lines().collect();
    if lines.first() != Some(&"*** Begin Patch") || lines.last() != Some(&"*** End Patch") {
        return Err(invalid("补丁必须完整包含 Begin Patch 与 End Patch"));
    }
    let mut actions = Vec::new();
    let mut index = 1;
    let end = lines.len() - 1;
    while index < end {
        let header = lines[index];
        index += 1;
        if let Some(path) = header.strip_prefix("*** Add File: ") {
            let mut content = String::new();
            while index < end && !lines[index].starts_with("*** ") {
                content.push_str(
                    lines[index]
                        .strip_prefix('+')
                        .ok_or_else(|| invalid("新增行必须以 + 开头"))?,
                );
                content.push('\n');
                index += 1;
            }
            actions.push(Action::Add {
                path: path.into(),
                content,
            });
        } else if let Some(path) = header.strip_prefix("*** Delete File: ") {
            actions.push(Action::Delete { path: path.into() });
        } else if let Some(path) = header.strip_prefix("*** Update File: ") {
            let target = lines
                .get(index)
                .and_then(|line| line.strip_prefix("*** Move to: "))
                .map(str::to_owned);
            if target.is_some() {
                index += 1;
            }
            let mut chunks = Vec::new();
            while index < end && (lines[index] == "@@" || lines[index].starts_with("@@ ")) {
                let anchor = lines[index].strip_prefix("@@ ").map(str::to_owned);
                index += 1;
                let mut chunk = Chunk {
                    anchor,
                    old: Vec::new(),
                    new: Vec::new(),
                    eof: false,
                };
                while index < end
                    && !lines[index].starts_with("*** ")
                    && lines[index] != "@@"
                    && !lines[index].starts_with("@@ ")
                {
                    let line = lines[index];
                    match line.as_bytes().first() {
                        Some(b' ') => {
                            chunk.old.push(line[1..].into());
                            chunk.new.push(line[1..].into());
                        }
                        Some(b'-') => chunk.old.push(line[1..].into()),
                        Some(b'+') => chunk.new.push(line[1..].into()),
                        _ => return Err(invalid("上下文行必须以空格、+ 或 - 开头")),
                    }
                    index += 1;
                }
                if lines.get(index) == Some(&"*** End of File") {
                    chunk.eof = true;
                    index += 1;
                }
                if chunk.old.is_empty() && chunk.new.is_empty() {
                    return Err(invalid("上下文块不能为空"));
                }
                chunks.push(chunk);
            }
            if chunks.is_empty() {
                return Err(invalid("Update File 需要至少一个 @@ 上下文块"));
            }
            actions.push(Action::Update {
                path: path.into(),
                target,
                chunks,
            });
        } else {
            return Err(invalid(format!("无法识别补丁动作: {header}")));
        }
        if actions.len() > MAX_ACTIONS {
            return Err(invalid("补丁最多包含 64 个动作"));
        }
    }
    if actions.is_empty() {
        return Err(invalid("补丁没有文件动作"));
    }
    Ok(actions)
}
