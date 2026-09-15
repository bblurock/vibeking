//! macOS permission status checks + Settings deep-link helpers.
//!
//! Accessibility is required to inject the paste keystroke (clipboard sandwich).
//! Input Monitoring is required for the CGEventTap to receive keystrokes from
//! other applications — without it the push-to-talk hotkey only fires while
//! Vibeking itself is focused.
//! Microphone is required to capture audio; macOS auto-prompts on first capture.

use serde::Serialize;

#[derive(Debug, Clone, Serialize)]
pub struct PermissionsStatus {
    pub accessibility: bool,
    pub input_monitoring: bool,
    pub microphone: bool,
}

#[tauri::command]
pub fn check_permissions() -> PermissionsStatus {
    #[cfg(target_os = "macos")]
    {
        PermissionsStatus {
            accessibility: mac::check_accessibility(),
            input_monitoring: mac::check_input_monitoring(),
            microphone: mac::check_microphone(),
        }
    }
    #[cfg(not(target_os = "macos"))]
    PermissionsStatus {
        accessibility: true,
        input_monitoring: true,
        microphone: true,
    }
}

#[tauri::command]
pub fn open_settings_pane(pane: String) -> Result<(), String> {
    let url = match pane.as_str() {
        "accessibility" => {
            "x-apple.systempreferences:com.apple.preference.security?Privacy_Accessibility"
        }
        "input-monitoring" => {
            "x-apple.systempreferences:com.apple.preference.security?Privacy_ListenEvent"
        }
        "microphone" => {
            "x-apple.systempreferences:com.apple.preference.security?Privacy_Microphone"
        }
        _ => return Err(format!("unknown settings pane: {pane}")),
    };

    #[cfg(target_os = "macos")]
    {
        std::process::Command::new("/usr/bin/open")
            .arg(url)
            .spawn()
            .map_err(|e| format!("open {url}: {e}"))?;
        Ok(())
    }
    #[cfg(not(target_os = "macos"))]
    {
        let _ = url;
        Err("settings panes are macOS-only".into())
    }
}

#[cfg(target_os = "macos")]
mod mac {
    use objc2::msg_send;
    use objc2::runtime::AnyClass;
    use objc2_foundation::NSString;

    #[link(name = "ApplicationServices", kind = "framework")]
    extern "C" {
        fn AXIsProcessTrusted() -> bool;
    }

    #[link(name = "IOKit", kind = "framework")]
    extern "C" {
        fn IOHIDCheckAccess(request_type: u32) -> u32;
    }

    // Force AVFoundation to be linked so AVCaptureDevice is resolvable at runtime.
    #[link(name = "AVFoundation", kind = "framework")]
    extern "C" {}

    // kIOHIDRequestTypeListenEvent = 1, kIOHIDAccessTypeGranted = 0
    const REQUEST_TYPE_LISTEN_EVENT: u32 = 1;
    const ACCESS_TYPE_GRANTED: u32 = 0;
    // AVAuthorizationStatusAuthorized = 3
    const AV_AUTH_AUTHORIZED: isize = 3;

    pub fn check_accessibility() -> bool {
        unsafe { AXIsProcessTrusted() }
    }

    pub fn check_input_monitoring() -> bool {
        unsafe { IOHIDCheckAccess(REQUEST_TYPE_LISTEN_EVENT) == ACCESS_TYPE_GRANTED }
    }

    pub fn check_microphone() -> bool {
        // [AVCaptureDevice authorizationStatusForMediaType:AVMediaTypeAudio]
        // AVMediaTypeAudio's underlying NSString value is "soun" (legacy QuickTime FourCC).
        let Some(cls) = AnyClass::get(c"AVCaptureDevice") else {
            log::info!("[vibeking] AVCaptureDevice class not found at runtime");
            return false;
        };
        let media_type = NSString::from_str("soun");
        let status: isize =
            unsafe { msg_send![cls, authorizationStatusForMediaType: &*media_type] };
        status == AV_AUTH_AUTHORIZED
    }
}
