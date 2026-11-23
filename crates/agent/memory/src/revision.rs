use crate::{
    MemoryClient, consolidation::sha256, publication::read_bounded, redaction::Redactor,
    retrieval::check_cancel, store::check_path,
};
use agentelier_tools::ToolUserTurn;
use regex::Regex;
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;
use std::fs;
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};
use tempfile::NamedTempFile;
use tokio_util::sync::CancellationToken;

const NOTE_BYTES: usize = 12 * 1024;
const REVISION_INPUT_BYTES: usize = 256 * 1024;
const TEXT_BYTES: usize = 1024;
const NOTE_HEADER: &str = "# Long-term memory revision\n\n```json\n";
const NOTE_FOOTER: &str = "\n```\n";
pub(crate) const TEMP_PREFIX: &str = ".agentelier-memory-note-";

#[derive(Clone, Copy, PartialEq, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum Action {
    Remember,
    Correct,
    Forget,
}

#[derive(Deserialize)]
#[serde(rename_all = "snake_case")]
enum ScopeArgument {
    Project,
    Global,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct RevisionArguments {
    action: Action,
    scope: ScopeArgument,
    target: String,
    replacement: Option<String>,
    user_request: String,
}

#[derive(Deserialize, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
enum RevisionScope {
    Global,
    Project { directory: String },
}

#[derive(Deserialize, Serialize)]
pub(crate) struct Revision {
    pub(crate) id: String,
    #[serde(default)]
    epoch: i64,
    pub(crate) action: Action,
    pub(crate) target: String,
    replacement: Option<String>,
    scope: RevisionScope,
    session_id: uuid::Uuid,
    turn: u64,
    input: u64,
    at_ms: u64,
    user_request: String,
}

#[derive(Serialize)]
pub(crate) struct RevisionReceipt {
    revision_id: String,
    state: &'static str,
}

impl Revision {
    pub(crate) fn applies_to(&self, directory: &str) -> bool {
        match &self.scope {
            RevisionScope::Global => true,
            RevisionScope::Project { directory: scope } => Path::new(scope) == Path::new(directory),
        }
    }

    pub(crate) fn affects_details(&self, directory: &str) -> bool {
        self.action != Action::Remember && self.applies_to(directory)
    }

    fn key(&self) -> String {
        let statement = format!("{}:{}:{}", self.session_id, self.input, self.user_request);
        if self.epoch == 0 {
            return sha256(&statement);
        }
        sha256(&format!("{}:{statement}", self.epoch))
    }
}

impl MemoryClient {
    pub async fn revision_requested(&self) {
        self.revision_signal.notified().await;
    }

    pub(crate) async fn revise(
        &self,
        turn: &ToolUserTurn,
        directory: &Path,
        arguments: RevisionArguments,
        cancel: CancellationToken,
    ) -> io::Result<RevisionReceipt> {
        let epoch = turn
            .memory_epoch
            .filter(|epoch| Some(*epoch) == self.epoch())
            .ok_or_else(|| {
                io::Error::new(
                    io::ErrorKind::PermissionDenied,
                    "该请求早于当前记忆边界，请重新提交修订",
                )
            })?;
        let redactor = Redactor::new()?;
        let user_request = validated(arguments.user_request, &redactor)?;
        if !turn.statements.iter().any(|statement| {
            statement.trim() == user_request
                || statement.lines().any(|line| line.trim() == user_request)
        }) || !authorizes(
            &user_request,
            arguments.action,
            &arguments.scope,
            turn.memory_management,
        )? {
            return Err(io::Error::new(
                io::ErrorKind::PermissionDenied,
                "修订需要当前用户明确提出的记住、纠正或忘记请求",
            ));
        }
        if (arguments.action == Action::Correct) != arguments.replacement.is_some() {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "纠正需要提供替代说法",
            ));
        }
        let target = validated(arguments.target, &redactor)?;
        let replacement = arguments
            .replacement
            .map(|text| validated(text, &redactor))
            .transpose()?;
        let authorized = user_request.to_lowercase();
        if !authorized.contains(&target.to_lowercase())
            || replacement
                .as_ref()
                .is_some_and(|text| !authorized.contains(&text.to_lowercase()))
        {
            return Err(io::Error::new(
                io::ErrorKind::PermissionDenied,
                "修订目标与替代说法需来自授权原话",
            ));
        }
        let scope = match arguments.scope {
            ScopeArgument::Global => RevisionScope::Global,
            ScopeArgument::Project => RevisionScope::Project {
                directory: directory
                    .to_str()
                    .ok_or_else(|| io::Error::other("当前工作目录无法存档"))?
                    .to_owned(),
            },
        };
        let mut note = Revision {
            id: String::new(),
            epoch,
            action: arguments.action,
            target,
            replacement,
            scope,
            session_id: turn.session,
            turn: turn.turn,
            input: turn.input,
            at_ms: u64::try_from(
                SystemTime::now()
                    .duration_since(UNIX_EPOCH)
                    .map_err(io::Error::other)?
                    .as_millis(),
            )
            .map_err(io::Error::other)?,
            user_request,
        };
        note.id = note.key();
        let (receipt, added) = self.with_active_store(move |connection, workspace| {
            let current = crate::reset::available_epoch(connection)?;
            let cutoff = crate::reset::last_turn(connection, &note.session_id.to_string())?;
            if current != Some(epoch) || note.input <= cutoff {
                return Err(io::Error::new(io::ErrorKind::PermissionDenied, "该用户输入早于当前记忆边界，请重新提交修订"));
            }
            check_cancel(&cancel)?;
            let folder = notes_directory(workspace, true)?;
            let path = folder.join(format!("{}.md", note.id));
            if let Some(saved) = read_note(&path, &note.id)? {
                let published: bool = connection.query_row(
                    "SELECT EXISTS(SELECT 1 FROM revisions WHERE revision_id = ?1 AND published = 1)",
                    [&saved.id], |row| row.get(0),
                ).map_err(io::Error::other)?;
                return Ok((RevisionReceipt {
                    revision_id: saved.id,
                    state: if published { "published" } else { "saved_pending_consolidation" },
                }, false));
            }
            let notes = revisions(workspace, epoch)?;
            let bytes = serde_json::to_vec(&notes).map_err(io::Error::other)?.len();
            let body = serde_json::to_string(&note).map_err(io::Error::other)?;
            if bytes.saturating_add(body.len()).saturating_add(1) > REVISION_INPUT_BYTES {
                return Err(io::Error::other("长期记忆修订总量已达整合预算"));
            }
            let content = format!("{NOTE_HEADER}{body}{NOTE_FOOTER}");
            if content.len() > NOTE_BYTES {
                return Err(io::Error::new(io::ErrorKind::InvalidInput, "修订笔记超出大小限制"));
            }
            let mut temporary = NamedTempFile::with_prefix_in(TEMP_PREFIX, &folder)?;
            temporary.write_all(content.as_bytes())?;
            temporary.as_file().sync_all()?;
            check_cancel(&cancel)?;
            temporary.persist_noclobber(path).map_err(|failure| failure.error)?;
            Ok((RevisionReceipt { revision_id: note.id, state: "saved_pending_consolidation" }, true))
        }).await?;
        if added {
            tracing::info!(revision_id = receipt.revision_id, session_id = %turn.session, turn = turn.turn, "用户长期记忆修订已保存，等待整合");
            self.revision_signal.notify_one();
        }
        Ok(receipt)
    }
}

fn authorizes(
    request: &str,
    action: Action,
    scope: &ScopeArgument,
    management: bool,
) -> io::Result<bool> {
    let refers_to_memory =
        Regex::new(r"(?i)\b(?:memory|memories|memorize)\b|长期记忆|从记忆|记忆中")
            .map_err(io::Error::other)?
            .is_match(request);
    let verb = match (action, refers_to_memory) {
        (Action::Remember, false) => r"remember\b|记住|记下",
        (Action::Remember, true) => r"remember\b|retain\b|save\b|record\b|记住|记下|记录|保存",
        (Action::Correct, false) => r"correct\b|更正|纠正|修正|改记",
        (Action::Correct, true) => r"correct\b|update\b|revise\b|change\b|更正|纠正|修正|更新|改记",
        (Action::Forget, false) => r"forget\b|忘记|忘掉",
        (Action::Forget, true) => {
            r"forget\b|remove\b|erase\b|delete\b|drop\b|忘记|忘掉|删除|移除|清除"
        }
    };
    let directive = Regex::new(&format!(
        r"(?im)(?:^|[.!?]\s+|[。！？]\s*)[ \t]*(?:(?:please|(?:can|could|would) you|i (?:want|need) you to|i(?:'d| would) like you to)[ \t]+|请(?:你|帮我)?\s*|帮我\s*)?(?:{verb})"
    )).map_err(io::Error::other)?;
    if !directive.is_match(request) {
        return Ok(false);
    }
    if management {
        return Ok(matches!(scope, ScopeArgument::Global));
    }
    if matches!(scope, ScopeArgument::Global) {
        let explicit_global = Regex::new(r"(?i)\b(?:globally|cross.project|across (?:all )?projects|(?:all|every) (?:project|repo|workspace)s?)\b|全局|跨项目|所有项目|每个项目").map_err(io::Error::other)?;
        return Ok(explicit_global.is_match(request));
    }
    Ok(true)
}

fn validated(text: String, redactor: &Redactor) -> io::Result<String> {
    let text = text.trim();
    if text.is_empty()
        || text.len() > TEXT_BYTES
        || text.contains('\0')
        || redactor.clean(text, TEXT_BYTES) != text
    {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "修订文字为空、超过限制或包含凭据",
        ));
    }
    Ok(text.to_owned())
}

pub(crate) fn notes_directory(workspace: &Path, create: bool) -> io::Result<PathBuf> {
    check_path(workspace, true)?;
    let mut directory = workspace.to_owned();
    for segment in ["extensions", "ad_hoc", "notes"] {
        directory.push(segment);
        if create {
            match fs::create_dir(&directory) {
                Ok(()) => {}
                Err(failure) if failure.kind() == io::ErrorKind::AlreadyExists => {}
                Err(failure) => return Err(failure),
            }
        }
        check_path(&directory, true)?;
    }
    Ok(directory)
}

pub(crate) fn revisions(workspace: &Path, epoch: i64) -> io::Result<Vec<Revision>> {
    let directory = notes_directory(workspace, false)?;
    let entries = match fs::read_dir(directory) {
        Ok(entries) => entries,
        Err(failure) if failure.kind() == io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(failure) => return Err(failure),
    };
    let mut notes = Vec::new();
    let mut bytes = 0usize;
    for entry in entries {
        let entry = entry?;
        let filename = entry.file_name();
        let Some(id) = filename
            .to_str()
            .and_then(|name| name.strip_suffix(".md"))
            .filter(|id| revision_id(id))
        else {
            continue;
        };
        let note = read_note(&entry.path(), id)?
            .ok_or_else(|| io::Error::other("长期记忆修订笔记已丢失"))?;
        if note.epoch != epoch {
            continue;
        }
        bytes =
            bytes.saturating_add(serde_json::to_vec(&note).map_err(io::Error::other)?.len() + 1);
        if bytes > REVISION_INPUT_BYTES {
            return Err(io::Error::other("长期记忆修订超出整合预算"));
        }
        notes.push(note);
    }
    notes.sort_by(|left, right| (left.at_ms, &left.id).cmp(&(right.at_ms, &right.id)));
    Ok(notes)
}

pub(crate) fn published_ids(connection: &rusqlite::Connection) -> io::Result<BTreeSet<String>> {
    let mut statement = connection
        .prepare("SELECT revision_id FROM revisions WHERE published = 1")
        .map_err(io::Error::other)?;
    statement
        .query_map([], |row| row.get(0))
        .map_err(io::Error::other)?
        .collect::<rusqlite::Result<_>>()
        .map_err(io::Error::other)
}

fn read_note(path: &Path, id: &str) -> io::Result<Option<Revision>> {
    let content = match read_bounded(path, NOTE_BYTES) {
        Ok(content) => content,
        Err(failure) if failure.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(failure) => return Err(failure),
    };
    let body = content
        .strip_prefix(NOTE_HEADER)
        .and_then(|body| body.strip_suffix(NOTE_FOOTER))
        .ok_or_else(|| io::Error::other("长期记忆修订笔记格式无效"))?;
    let note: Revision = serde_json::from_str(body).map_err(io::Error::other)?;
    if note.id != id || note.key() != id {
        return Err(io::Error::other("长期记忆修订身份校验失败"));
    }
    Ok(Some(note))
}

pub(crate) fn revision_id(id: &str) -> bool {
    const DIGEST_BYTES: usize = 64;
    id.len() == DIGEST_BYTES
        && id
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}
