use super::{
    invalid,
    prepare::{Prepared, unchanged},
};
use crate::{
    ToolFailure, ToolResult, ToolStatus,
    blocking::{self, Control},
};
use serde_json::{Value, json};
use std::{
    fs,
    io::{self, Write},
};

const ERROR_BYTES: usize = 512;
const REPORT_RESERVE: usize = 4096;

fn record(action: &Prepared) -> Value {
    json!({
        "source":action.source,"target":action.target,
        "kind":if action.old.is_none() {"add"} else if action.new.is_none() {"delete"} else if action.source != action.target {"move"} else {"update"},
        "status":"not_started","target_written":false,"source_removed":false,
        "created_directories":[],"temporary_file":null,"temporary_removed":false,"error":null
    })
}

pub(super) fn check_report_budget(actions: &[Prepared]) -> Result<(), ToolFailure> {
    let estimated: Vec<_> = actions
        .iter()
        .map(|action| {
            let mut record = record(action);
            record["created_directories"] = json!(action.directories);
            record["temporary_file"] = json!(action.target.with_file_name(format!(
                    "{}.temporary-XXXXXXXXXXXXXXXX",
                    action
                        .target
                        .file_name()
                        .unwrap_or_default()
                        .to_string_lossy()
                )));
            record
        })
        .collect();
    if json!(estimated).to_string().len() + REPORT_RESERVE > crate::MAX_TOOL_BODY_BYTES {
        return Err(invalid("动作报告无法装入 64 KiB，未修改文件"));
    }
    Ok(())
}

fn short(error: impl std::fmt::Display) -> String {
    let mut text = error.to_string();
    let mut end = ERROR_BYTES.min(text.len());
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    text.truncate(end);
    text
}

fn confirm(action: &Prepared) -> Result<(), ToolFailure> {
    if let Some(old) = &action.old {
        if !unchanged(&action.source, old)? {
            return Err(invalid("准备后源文件已经改变，未提交当前动作"));
        }
    } else if action
        .source
        .try_exists()
        .map_err(|error| invalid(error.to_string()))?
    {
        return Err(invalid("准备后新增目标已经出现"));
    }
    if action.source != action.target
        && action
            .target
            .try_exists()
            .map_err(|error| invalid(error.to_string()))?
    {
        return Err(invalid("准备后移动目标已经出现"));
    }
    Ok(())
}

fn apply(action: &Prepared, record: &mut Value) -> Result<(), ToolFailure> {
    confirm(action)?;
    if let Some(new) = &action.new {
        for directory in &action.directories {
            match fs::create_dir(directory) {
                Ok(()) => {
                    if let Some(created) = record["created_directories"].as_array_mut() {
                        created.push(json!(directory));
                    }
                }
                Err(error)
                    if error.kind() == io::ErrorKind::AlreadyExists && directory.is_dir() => {}
                Err(error) => return Err(invalid(error.to_string())),
            }
        }
        let parent = action
            .target
            .parent()
            .ok_or_else(|| invalid("目标没有父目录"))?;
        let mut temp =
            tempfile::NamedTempFile::new_in(parent).map_err(|error| invalid(error.to_string()))?;
        record["temporary_file"] = json!(temp.path());
        let written = temp.write_all(new).and_then(|()| temp.as_file().sync_all());
        if let Err(error) = written {
            record["error"] = json!(short(&error));
            cleanup(temp, record)?;
            return Err(invalid(error.to_string()));
        }
        if let Err(error) = confirm(action) {
            record["error"] = json!(short(&error));
            cleanup(temp, record)?;
            return Err(error);
        }
        let result = if action.old.is_some() && action.source == action.target {
            temp.persist(&action.target)
        } else {
            temp.persist_noclobber(&action.target)
        };
        match result {
            Ok(_) => {
                record["target_written"] = json!(true);
                record["temporary_removed"] = json!(true);
            }
            Err(error) => {
                let reason = short(&error.error);
                record["error"] = json!(reason);
                cleanup(error.file, record)?;
                return Err(invalid(reason));
            }
        }
    }
    if action.new.is_none() || action.source != action.target {
        if let Some(old) = &action.old
            && !unchanged(&action.source, old)?
        {
            return Err(invalid("源文件已改变，目标已写入但源文件保留"));
        }
        fs::remove_file(&action.source).map_err(|error| invalid(error.to_string()))?;
        record["source_removed"] = json!(true);
    }
    Ok(())
}

fn cleanup(temp: tempfile::NamedTempFile, record: &mut Value) -> Result<(), ToolFailure> {
    match temp.close() {
        Ok(()) => {
            record["temporary_removed"] = json!(true);
            Ok(())
        }
        Err(error) => {
            record["cleanup_error"] = json!(short(&error));
            Err(ToolFailure::new("temporary_cleanup_failed", short(error)))
        }
    }
}

pub(super) fn commit(actions: Vec<Prepared>, control: &Control) -> ToolResult {
    let mut records: Vec<_> = actions.iter().map(record).collect();
    let mut failure = None;
    let mut cleanup_failed = false;
    for (action, record) in actions.iter().zip(&mut records) {
        if let Err(error) = control.check() {
            failure = Some(error);
            break;
        }
        record["status"] = json!("started");
        match apply(action, record) {
            Ok(()) => record["status"] = json!("completed"),
            Err(error) => {
                cleanup_failed = error.code == "temporary_cleanup_failed";
                record["status"] = json!("failed");
                if record["error"].is_null() {
                    record["error"] = json!(short(&error));
                }
                failure = Some(error);
                break;
            }
        }
    }
    if failure.is_none() {
        failure = control.check().err();
    }
    let status = failure
        .as_ref()
        .map_or(ToolStatus::Succeeded, |error| blocking::status(error.code));
    let error = failure.map(|error| json!({"code":error.code,"message":short(&error)}));
    ToolResult {
        status,
        cleanup_failed,
        output: json!({"ok":error.is_none(),"actions":records,"error":error}).to_string(),
    }
}
