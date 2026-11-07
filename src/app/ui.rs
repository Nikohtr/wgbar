//! Small AppKit / UserNotifications helpers shared by the delegate.

use block2::RcBlock;
use objc2::rc::Retained;
use objc2::runtime::{Bool, Sel};
use objc2::{MainThreadMarker, MainThreadOnly};
use objc2_app_kit::{NSAlert, NSAlertStyle, NSApplication, NSImage, NSMenuItem};
use objc2_foundation::{NSBundle, NSError, NSString, NSUUID};
use objc2_user_notifications::{
    UNAuthorizationOptions, UNMutableNotificationContent, UNNotificationRequest,
    UNUserNotificationCenter,
};

pub fn ns(s: &str) -> Retained<NSString> {
    NSString::from_str(s)
}

/// Swift `NSMenuItem(title:action:keyEquivalent:)`.
pub fn menu_item(
    mtm: MainThreadMarker,
    title: &str,
    action: Option<Sel>,
    key: &str,
) -> Retained<NSMenuItem> {
    // SAFETY: every selector passed here is a method of `AppDelegate` or `terminate:` of
    // NSApplication, all taking one object argument (the sender) and returning nothing.
    unsafe {
        NSMenuItem::initWithTitle_action_keyEquivalent(
            NSMenuItem::alloc(mtm),
            &ns(title),
            action,
            &ns(key),
        )
    }
}

/// An SF Symbol as a template image, so the menu bar tints it for light/dark mode.
pub fn symbol(name: &str, description: &str) -> Option<Retained<NSImage>> {
    let image = NSImage::imageWithSystemSymbolName_accessibilityDescription(
        &ns(name),
        Some(&ns(description)),
    );
    if let Some(image) = &image {
        image.setTemplate(true);
    }
    image
}

/// Bring the (accessory) app forward so its alert or panel is not hidden behind other apps.
pub fn activate(mtm: MainThreadMarker) {
    // `activate()` needs macOS 14 and the app supports 13 (LSMinimumSystemVersion), so keep
    // the call the Swift app made.
    #[allow(deprecated)]
    NSApplication::sharedApplication(mtm).activateIgnoringOtherApps(true);
}

/// A modal alert with one OK button.
pub fn alert(mtm: MainThreadMarker, title: &str, message: &str, style: NSAlertStyle) {
    let alert = NSAlert::new(mtm);
    alert.setAlertStyle(style);
    alert.setMessageText(&ns(title));
    alert.setInformativeText(&ns(message));
    activate(mtm);
    alert.runModal();
}

/// Post a user notification titled "WGBar". Silently does nothing when the user declined
/// notifications or the binary runs outside an app bundle.
pub fn notify(body: &str) {
    // UNUserNotificationCenter needs a bundle (it throws when there is none).
    if NSBundle::mainBundle().bundleIdentifier().is_none() {
        return;
    }
    let body = body.to_owned();
    // Called on an arbitrary queue; it only touches thread-safe UserNotifications objects.
    let completion = RcBlock::new(move |granted: Bool, _error: *mut NSError| {
        if !granted.as_bool() {
            return;
        }
        let content = UNMutableNotificationContent::new();
        content.setTitle(&ns("WGBar"));
        content.setBody(&ns(&body));
        let request = UNNotificationRequest::requestWithIdentifier_content_trigger(
            &NSUUID::UUID().UUIDString(),
            &content,
            None,
        );
        UNUserNotificationCenter::currentNotificationCenter()
            .addNotificationRequest_withCompletionHandler(&request, None);
    });
    UNUserNotificationCenter::currentNotificationCenter()
        .requestAuthorizationWithOptions_completionHandler(
            UNAuthorizationOptions::Alert,
            &completion,
        );
}
