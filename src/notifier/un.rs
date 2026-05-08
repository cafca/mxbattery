use objc2::runtime::Bool;
use objc2_foundation::NSError;
use objc2_foundation::NSString;
use objc2_user_notifications::{
    UNAuthorizationOptions, UNMutableNotificationContent, UNNotificationRequest,
    UNUserNotificationCenter,
};

pub fn request_permission() {
    let center = unsafe { UNUserNotificationCenter::currentNotificationCenter() };
    let opts = UNAuthorizationOptions::UNAuthorizationOptionAlert
        | UNAuthorizationOptions::UNAuthorizationOptionSound;
    let block = block2::RcBlock::new(|_granted: Bool, _err: *mut NSError| {});
    unsafe {
        center.requestAuthorizationWithOptions_completionHandler(opts, &block);
    }
}

pub fn post(title: &str, body: &str, identifier: &str) {
    let center = unsafe { UNUserNotificationCenter::currentNotificationCenter() };
    let content = unsafe { UNMutableNotificationContent::new() };
    unsafe {
        content.setTitle(&NSString::from_str(title));
        content.setBody(&NSString::from_str(body));
    }
    let req = unsafe {
        UNNotificationRequest::requestWithIdentifier_content_trigger(
            &NSString::from_str(identifier),
            &content,
            None,
        )
    };
    let block = block2::RcBlock::new(|_err: *mut NSError| {});
    unsafe {
        center.addNotificationRequest_withCompletionHandler(&req, Some(&block));
    }
}
