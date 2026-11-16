use std::sync::Arc;

pub const MAX_SESSION_IMAGE_BYTES: usize = 16 * 1024 * 1024;

pub struct ImagePayload {
    pub(crate) bytes: Vec<u8>,
    pub(crate) mime: &'static str,
    pub(crate) digest: String,
    pub(crate) width: u32,
    pub(crate) height: u32,
}
impl ImagePayload {
    pub fn bytes(&self) -> &[u8] {
        &self.bytes
    }
    pub fn mime(&self) -> &'static str {
        self.mime
    }
    pub fn digest(&self) -> &str {
        &self.digest
    }
    pub fn dimensions(&self) -> (u32, u32) {
        (self.width, self.height)
    }
}

#[derive(Clone)]
pub enum ToolPayload {
    Text(String),
    Image {
        summary: String,
        image: Arc<ImagePayload>,
    },
}
impl From<String> for ToolPayload {
    fn from(value: String) -> Self {
        Self::Text(value)
    }
}
impl ToolPayload {
    pub fn summary(&self) -> &str {
        match self {
            Self::Text(text) => text,
            Self::Image { summary, .. } => summary,
        }
    }
    pub fn image(&self) -> Option<&Arc<ImagePayload>> {
        match self {
            Self::Image { image, .. } => Some(image),
            Self::Text(_) => None,
        }
    }
}
