use agentelier_tools::{ImageDescriptor, ImagePayload};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::{collections::HashMap, ops::Range, sync::Arc};
use uuid::Uuid;

#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct UserImage {
    pub session: Uuid,
    pub id: Uuid,
    pub name: String,
    pub image: ImageDescriptor,
    pub placeholder: Range<usize>,
}

#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct UserMessage {
    pub text: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub images: Vec<UserImage>,
}

impl UserMessage {
    pub fn text(text: impl Into<String>) -> Self {
        Self {
            text: text.into(),
            images: Vec::new(),
        }
    }

    pub fn image_label(number: usize) -> String {
        format!("[Image #{number}]")
    }

    pub(super) fn validate(&self) -> Result<(), crate::ModelFailure> {
        let mut end = 0;
        for (index, image) in self.images.iter().enumerate() {
            if image.session.is_nil()
                || image.id.is_nil()
                || image.name.is_empty()
                || image.name.contains(['/', '\\', '"'])
                || image.name.contains(char::is_control)
                || image.placeholder.start < end
                || self.text.get(image.placeholder.clone())
                    != Some(Self::image_label(index + 1).as_str())
            {
                return Err(crate::model::protocol("用户图片的身份或占位符无效"));
            }
            image
                .image
                .validate()
                .map_err(|failure| crate::model::protocol(&failure.to_string()))?;
            end = image.placeholder.end;
        }
        Ok(())
    }

    pub(super) fn into_input(
        self,
        bytes: &HashMap<(Uuid, Uuid), Arc<ImagePayload>>,
    ) -> Result<Value, crate::ModelFailure> {
        self.validate()?;
        let mut content = Vec::with_capacity(self.images.len() * 3 + 1);
        for (index, image) in self.images.iter().enumerate() {
            let payload = bytes
                .get(&(image.session, image.id))
                .filter(|payload| payload.descriptor() == image.image)
                .ok_or_else(|| {
                    crate::model::protocol(&format!("用户图片没有一致的正式附件：{}", image.name))
                })?;
            use base64::Engine;
            let url = format!(
                "data:{};base64,{}",
                payload.mime(),
                base64::engine::general_purpose::STANDARD.encode(payload.bytes())
            );
            let number = index + 1;
            let label = Self::image_label(number);
            content.push(json!({"type":"input_text","text":format!("<image name={label} path=\"{}\">", image.name)}));
            content.push(json!({"type":"input_image","image_url":url,"detail":"high"}));
            content.push(json!({"type":"input_text","text":"</image>"}));
            tracing::debug!(attachment_id = %image.id, number, mime = payload.mime(), sha256 = payload.digest(), encoded_bytes = payload.bytes().len(), "用户图片已投影至模型请求");
        }
        content.push(json!({"type":"input_text","text":self.text}));
        Ok(json!({"role":"user","content":content}))
    }
}
