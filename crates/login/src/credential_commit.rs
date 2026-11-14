use crate::authentication::{AccountRecord, AuthFile, RestoredAccount};
use crate::browser::{AuthorizationContext, LoginFailure, LoginStage, VerifiedLogin};
use std::fs;
use std::io::{self, Write};
use std::os::windows::fs::MetadataExt;
use std::path::Path;
use tempfile::{NamedTempFile, TempPath};
use win_custody::{
    ObjectSecurityDescriptor, OwnedSid, SecurityDescriptor, current_user_sid_string,
};

const REPARSE_POINT: u32 = 0x400;
const PROTECTED_DACL: u16 = 0x1000;

impl AuthFile {
    pub fn prepare_login(&mut self) -> Result<AuthorizationContext, LoginFailure> {
        if self.pending.is_some() {
            return Err(LoginFailure::new(
                LoginStage::Save,
                "仍有待保存的登录结果，请先重试保存或取消",
            ));
        }
        self.restore()
            .map_err(|error| LoginFailure::storage(LoginStage::Prepare, error))?;
        if self.record.is_none() {
            let record = AccountRecord {
                ext_agent_host_id: uuid::Uuid::new_v4().urn().to_string(),
                registration: None,
                session: None,
            };
            commit(&self.path, &record, true)
                .map_err(|error| LoginFailure::storage(LoginStage::Prepare, error))?;
            self.record = Some(record);
        }
        let record = self
            .record
            .as_ref()
            .ok_or_else(|| LoginFailure::new(LoginStage::Prepare, "缺少已保存的主机记录"))?;
        Ok(AuthorizationContext {
            host_id: record.ext_agent_host_id.clone(),
            client_id: record
                .registration
                .as_ref()
                .map(|registration| registration.client_id.clone())
                .or_else(|| self.issued_client.clone()),
            subject: record
                .registration
                .as_ref()
                .map(|registration| registration.subject.clone()),
            email: record
                .registration
                .as_ref()
                .and_then(|registration| registration.email.clone()),
        })
    }

    pub fn remember_issued_client(&mut self, client_id: String) {
        self.issued_client = Some(client_id);
    }

    pub fn stage_login(&mut self, verified: VerifiedLogin) -> Result<(), LoginFailure> {
        let current = self
            .record
            .as_ref()
            .ok_or_else(|| LoginFailure::new(LoginStage::Save, "缺少当前主机记录"))?;
        if current.ext_agent_host_id != verified.record.ext_agent_host_id
            || current.registration.as_ref().is_some_and(|registration| {
                verified.record.registration.as_ref().is_none_or(|next| {
                    registration.client_id != next.client_id || registration.subject != next.subject
                })
            })
        {
            return Err(LoginFailure::new(
                LoginStage::Identity,
                "返回账号与当前登记不匹配，原会话保持不变",
            ));
        }
        verified
            .record
            .validate()
            .map_err(|error| LoginFailure::storage(LoginStage::Save, error))?;
        self.pending = Some(verified.record);
        Ok(())
    }

    pub fn save_pending(&mut self) -> Result<RestoredAccount, LoginFailure> {
        let record = self
            .pending
            .as_ref()
            .ok_or_else(|| LoginFailure::new(LoginStage::Save, "没有待保存的登录结果"))?;
        let projection = record
            .projection()
            .map_err(|error| LoginFailure::storage(LoginStage::Save, error))?;
        commit(&self.path, record, false)
            .map_err(|error| LoginFailure::storage(LoginStage::Save, error))?;
        self.record = self.pending.take();
        self.issued_client = None;
        Ok(projection)
    }

    pub fn discard_pending(&mut self) {
        self.pending = None;
    }
}

fn commit(path: &Path, record: &AccountRecord, create: bool) -> io::Result<()> {
    record.validate().map_err(io::Error::other)?;
    let content =
        serde_json::to_vec_pretty(record).map_err(|_| io::Error::other("无法序列化凭据记录"))?;
    let parent = path
        .parent()
        .ok_or_else(|| io::Error::other("凭据路径缺少父目录"))?;
    let sid = current_user_sid_string()?;
    let descriptor =
        SecurityDescriptor::from_sddl(&format!("D:P(A;OICI;FA;;;{sid})(A;OICI;FA;;;SY)"))?;
    match fs::symlink_metadata(path) {
        Ok(metadata) => {
            if !metadata.is_file() || metadata.file_attributes() & REPARSE_POINT != 0 {
                return Err(io::Error::other(
                    "凭据目标必须是普通文件，不能是目录或重解析点",
                ));
            }
            if create {
                return Err(io::Error::new(
                    io::ErrorKind::AlreadyExists,
                    "凭据文件已存在，保留原文件",
                ));
            }
            descriptor.apply_to_tree(path, true)?;
            verify(path, &sid)?;
        }
        Err(error) if error.kind() == io::ErrorKind::NotFound => {}
        Err(error) => return Err(error),
    }
    let directory = tempfile::Builder::new()
        .prefix(".agentelier-auth-")
        .tempdir_in(parent)?;
    descriptor.apply_to_tree(directory.path(), true)?;
    verify(directory.path(), &sid)?;
    let mut temporary = NamedTempFile::new_in(directory.path())?;
    descriptor.apply_to_tree(temporary.path(), true)?;
    verify(temporary.path(), &sid)?;
    temporary.write_all(&content)?;
    temporary.as_file().sync_all()?;
    // Move the protected file out so all directory cleanup precedes the commit point.
    let staging = parent.join(format!(".agentelier-auth-{}.tmp", uuid::Uuid::new_v4()));
    let file = temporary
        .persist_noclobber(&staging)
        .map_err(|error| error.error)?;
    let cleanup = TempPath::try_from_path(staging)?;
    let temporary = NamedTempFile::from_parts(file, cleanup);
    directory.close()?;
    verify(temporary.path(), &sid)?;
    if create {
        temporary
            .persist_noclobber(path)
            .map_err(|error| error.error)?;
    } else {
        temporary.persist(path).map_err(|error| error.error)?;
    }
    Ok(())
}

fn verify(path: &Path, sid: &str) -> io::Result<()> {
    let descriptor = ObjectSecurityDescriptor::query_file(path)?;
    if descriptor.control()? & PROTECTED_DACL == 0 {
        return Err(io::Error::other("凭据文件权限仍在继承"));
    }
    let owner = OwnedSid::from_string(sid)?;
    let system = OwnedSid::from_string("S-1-5-18")?;
    let acl = descriptor.dacl().map_err(io::Error::other)?;
    if acl.len() != 2
        || acl
            .allowed_aces()
            .any(|ace| !ace.trustee_matches(&owner) && !ace.trustee_matches(&system))
    {
        return Err(io::Error::other("凭据文件权限未限制为当前用户和 SYSTEM"));
    }
    Ok(())
}
