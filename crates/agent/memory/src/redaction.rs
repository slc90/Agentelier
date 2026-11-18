use regex::Regex;
use std::io;

pub(crate) struct Redactor(Regex);

impl Redactor {
    pub(crate) fn new() -> io::Result<Self> {
        Regex::new(concat!(
            r"(?im)(?:-----BEGIN [A-Z ]*PRIVATE KEY-----[\s\S]*?(?:-----END [A-Z ]*PRIVATE KEY-----|\z))",
            r#"|(?:\b(?:authorization|proxy-authorization|cookie|set-cookie|[\w.-]*(?:access[_-]?token|refresh[_-]?token|api[_-]?key|password|passwd|client[_-]?secret|session[_-]?token|secret[_-]?key|credential)[\w.-]*)\b["']?\s*[:=][^\r\n]*)"#,
            r"|(?:\bBearer[ \t]+[A-Za-z0-9_./+~=-]+)",
            r"|(?:\b(?:sk-[A-Za-z0-9_-]{12,}|gh[pousr]_[A-Za-z0-9_]{16,}|github_pat_[A-Za-z0-9_]{16,}|xox[baprs]-[A-Za-z0-9-]{12,}|(?:AKIA|ASIA)[A-Z0-9]{16}|eyJ[A-Za-z0-9_-]+\.[A-Za-z0-9_-]+\.[A-Za-z0-9_-]+))",
            r#"|(?:https?://[^\s<>"']*(?:[?&](?:token|code|signature|sig|key|password|x-amz-[a-z-]+)=|:[^/\s<>"']+@)[^\s<>"']*)"#
        )).map(Self).map_err(io::Error::other)
    }

    pub(crate) fn clean(&self, text: &str, limit: usize) -> String {
        let text = self.0.replace_all(text, "[已脱敏]");
        text[..text.floor_char_boundary(limit)].to_owned()
    }
}
