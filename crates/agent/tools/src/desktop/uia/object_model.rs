use super::elements::parse_reference;
use super::patterns::{get_pattern, optional_provider, pattern_result};
use super::{Runtime, native};
use crate::ToolFailure;
use serde_json::{Value, json};
use std::time::Instant;
use tokio_util::sync::CancellationToken;
use uiautomation::{
    errors::Error as UiaError,
    patterns::{UIPattern, UIPatternType},
};
use windows::{
    Win32::{System::Com::IDispatch, UI::Accessibility::IUIAutomationObjectModelPattern},
    core::{IUnknown, Interface},
};

struct ObjectModel(IUIAutomationObjectModelPattern);

impl UIPattern for ObjectModel {
    const TYPE: UIPatternType = UIPatternType::ObjectModel;
}

impl TryFrom<IUnknown> for ObjectModel {
    type Error = UiaError;

    fn try_from(pattern: IUnknown) -> Result<Self, Self::Error> {
        pattern.cast().map(Self).map_err(UiaError::from)
    }
}

impl Runtime {
    pub(crate) fn object_model(
        &self,
        reference: &str,
        cancel: &CancellationToken,
        deadline: Instant,
    ) -> Result<Value, ToolFailure> {
        let id = parse_reference(reference)?;
        self.call(cancel, deadline, move |worker, control| {
            let entry = worker.resolve_element(id, control)?;
            let pattern: ObjectModel = get_pattern(&entry.element, control)?;
            let model = optional_provider(control, || native::underlying_object_model(&pattern.0))?;
            let dispatch_supported = model
                .as_ref()
                .map(|model| model.cast::<IDispatch>().is_ok());
            Ok(pattern_result(
                id,
                "ObjectModel",
                "info",
                json!({"available": model.is_some(), "dispatch_supported": dispatch_supported}),
            ))
        })
    }
}
