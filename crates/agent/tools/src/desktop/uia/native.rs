use super::provider_error;
use crate::ToolFailure;
use uiautomation::{
    UIAutomation, UIElement, errors::Error as UiaError, patterns::UIItemContainerPattern,
    types::UIProperty, variants::Variant,
};
use windows::{
    Win32::UI::Accessibility::{
        IUIAutomation, IUIAutomationElement, IUIAutomationItemContainerPattern,
        IUIAutomationObjectModelPattern,
    },
    core::IUnknown,
};

#[allow(unsafe_code)]
pub(super) fn underlying_object_model(
    pattern: &IUIAutomationObjectModelPattern,
) -> Result<IUnknown, UiaError> {
    // SAFETY: The Worker owns and calls the pattern on the initialized MTA. The binding
    // initializes the output and retains the returned interface until the Worker drops it.
    unsafe { pattern.GetUnderlyingObjectModel() }.map_err(UiaError::from)
}

#[allow(unsafe_code)]
pub(super) fn property(
    automation: &UIAutomation,
    element: &UIElement,
    property: UIProperty,
    cached: bool,
) -> Result<Option<Variant>, ToolFailure> {
    let raw: &IUIAutomationElement = element.as_ref();
    // SAFETY: The initialized MTA worker owns both interfaces and keeps them alive for the calls.
    // The binding owns the out parameter until success, then Variant takes ownership.
    let value = unsafe {
        if cached {
            raw.GetCachedPropertyValueEx(property.into(), true)
        } else {
            raw.GetCurrentPropertyValueEx(property.into(), true)
        }
    }
    .map(Variant::from)
    .map_err(|error| provider_error(error.into()))?;
    Ok((!property_not_supported(automation, &value)?).then_some(value))
}

#[allow(unsafe_code)]
pub(super) fn property_not_supported(
    automation: &UIAutomation,
    value: &Variant,
) -> Result<bool, ToolFailure> {
    let automation: &IUIAutomation = automation.as_ref();
    // SAFETY: The VARIANT remains initialized and borrowed for this call on the same worker.
    let missing = unsafe { automation.CheckNotSupported(value.as_ref()) }
        .map_err(|error| provider_error(error.into()))?;
    Ok(missing.as_bool())
}

#[allow(unsafe_code)]
pub(super) fn find_item(
    pattern: &UIItemContainerPattern,
    after: Option<&UIElement>,
    property: UIProperty,
    value: &Variant,
) -> Result<UIElement, UiaError> {
    let raw: &IUIAutomationItemContainerPattern = pattern.as_ref();
    let after: Option<&IUIAutomationElement> = after.map(AsRef::as_ref);
    // SAFETY: All interfaces and the borrowed VARIANT stay alive on the initialized MTA worker.
    // A null start element is accepted by FindItemByProperty and starts at the first item.
    unsafe { raw.FindItemByProperty(after, property.into(), value.as_ref()) }
        .map(UIElement::from)
        .map_err(UiaError::from)
}
