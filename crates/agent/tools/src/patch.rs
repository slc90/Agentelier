mod commit;
mod parse;
mod prepare;

use crate::{ToolFailure, ToolResult, WorkingDirectory, blocking};
use serde::Deserialize;
use std::path::PathBuf;
use tokio_util::sync::CancellationToken;

const MAX_ACTIONS: usize = 64;
const MAX_FILE_BYTES: usize = 8 * 1024 * 1024;
const MAX_PREPARED_BYTES: usize = 32 * 1024 * 1024;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Arguments {
    patch: String,
    workdir: Option<PathBuf>,
}

fn invalid(message: impl Into<String>) -> ToolFailure {
    ToolFailure::new("invalid_patch", message)
}

pub(crate) async fn run(text: &str, directory: PathBuf, cancel: &CancellationToken) -> ToolResult {
    let input: Arguments = match crate::process::arguments(text, &["patch", "workdir"]) {
        Ok(input) => input,
        Err(error) => return blocking::failure(error),
    };
    blocking::run(cancel, move |control| {
        let prepared = (|| {
            control.check()?;
            let directory = WorkingDirectory::validate(&directory)
                .and_then(|base| base.directory(input.workdir.as_deref()))
                .map_err(|error| invalid(format!("工作目录无效: {error}")))?;
            let actions = parse::parse(&input.patch)?;
            prepare::prepare(actions, &directory, &control)
        })();
        match prepared {
            Ok(actions) => {
                tracing::debug!(action_count = actions.len(), "补丁准备完成，开始逐文件提交");
                commit::commit(actions, &control)
            }
            Err(error) => blocking::failure(error),
        }
    })
    .await
}
