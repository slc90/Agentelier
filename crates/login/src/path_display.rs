use std::path::{Component, Path, Prefix};

pub fn display_path(path: &Path) -> String {
    let mut components = path.components();
    if let Some(Component::Prefix(prefix)) = components.next() {
        match prefix.kind() {
            Prefix::VerbatimDisk(drive) => {
                return format!("{}:{}", char::from(drive), components.as_path().display());
            }
            Prefix::VerbatimUNC(server, share) => {
                return format!(
                    r"\\{}\{}{}",
                    server.to_string_lossy(),
                    share.to_string_lossy(),
                    components.as_path().display(),
                );
            }
            _ => {}
        }
    }
    path.display().to_string()
}
