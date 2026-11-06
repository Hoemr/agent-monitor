//! Minimal native macOS Accessibility (AX) + CoreGraphics bridge.
//!
//! Why not AppleScript: Chromium/Electron apps (DSH, VSCode) expose their AX
//! tree lazily and `tell application "System Events" … entire contents` misses
//! the sidebar rows. Reading `AXUIElement`s directly and clicking with a real
//! `CGEvent` is what works (verified on this host). No new crates: raw FFI to
//! ApplicationServices / CoreFoundation / CoreGraphics with small owning
//! wrappers; no subprocess helper is needed.
//!
//! Public surface (kept intentionally small):
//! * [`is_trusted`] — Accessibility grant for this process;
//! * [`focused_window_title`] / [`window_titles`] — AX title read-back;
//! * [`rows`] — rows of the first outline/table container (the app sidebar), in
//!   document order, with the text runs and their global screen frames. Later
//!   outlines in the window (e.g. Chromium chat content) are ignored;
//! * [`click`] — synthetic left click at a rectangle's centre, cursor restored.
//!
//! Everything is best effort: a missing grant, an app that is not running or a
//! window without an outline simply yields an empty result, never a panic.

use serde::Serialize;

#[derive(Clone, Debug, Default, Serialize)]
pub struct FocusedConversation {
    pub app_name: String,
    pub bundle_id: String,
    pub title: String,
    pub selected_titles: Vec<String>,
    pub references: Vec<String>,
    pub focused_web_title: Option<String>,
}

/// Foreground identity without an AX permission requirement.
pub fn foreground_bundle_id() -> Option<String> {
    #[cfg(target_os = "macos")]
    { platform::foreground_application().map(|(_, _, bundle)| bundle) }
    #[cfg(not(target_os = "macos"))]
    { None }
}

/// Foreground application only; never infer viewing from a background window.
pub fn focused_conversation(bundle_ids: &[String]) -> Option<FocusedConversation> {
    #[cfg(target_os = "macos")]
    { platform::focused_conversation(bundle_ids) }
    #[cfg(not(target_os = "macos"))]
    { let _ = bundle_ids; None }
}

/// Global screen rectangle, top-left origin (AX `AXPosition`/`AXSize` units).
#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize)]
pub struct Rect {
    pub x: f64,
    pub y: f64,
    pub w: f64,
    pub h: f64,
}

impl Rect {
    pub fn center(&self) -> (f64, f64) {
        (self.x + self.w / 2.0, self.y + self.h / 2.0)
    }

    /// Finite and non-empty; protects the CG click path from bogus AX values.
    pub fn is_sane(&self) -> bool {
        self.x.is_finite()
            && self.y.is_finite()
            && self.w.is_finite()
            && self.h.is_finite()
            && self.w >= 1.0
            && self.h >= 1.0
    }
}

/// One text run inside a row, e.g. the session title or its relative time.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct TextRun {
    pub text: String,
    pub frame: Option<Rect>,
}

/// One AX row (typically `AXOutlineRow` of the sidebar session list).
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Row {
    pub texts: Vec<TextRun>,
    pub frame: Option<Rect>,
}

impl Row {
    /// Trimmed, non-empty text runs in document order.
    pub fn text_strings(&self) -> Vec<String> {
        self.texts
            .iter()
            .map(|run| run.text.trim().to_string())
            .filter(|text| !text.is_empty())
            .collect()
    }
}

#[cfg(target_os = "macos")]
pub fn is_trusted() -> bool {
    platform::is_trusted()
}

/// Ask macOS to add the CURRENT executable to Accessibility (native prompt).
///
/// Returns `true` when already trusted (no prompt is shown in that case), else
/// `false` after the prompt was requested. Never blocks waiting for the user:
/// the grant is made in System Settings and read back by the next check.
#[cfg(target_os = "macos")]
pub fn request_accessibility_permission() -> Result<bool, String> {
    let trusted_before = platform::is_trusted();
    if trusted_before {
        return Ok(true);
    }
    let prompt_result = platform::prompt_accessibility_permission()?;
    Ok(resolve_prompt_result(trusted_before, prompt_result))
}

#[cfg(not(target_os = "macos"))]
pub fn request_accessibility_permission() -> Result<bool, String> {
    Err("accessibility_prompt_unsupported".to_string())
}

/// The prompt call returns the trust state at call time; combine it with the
/// pre-check so an already-trusted app can never regress to `false`.
#[cfg(target_os = "macos")]
fn resolve_prompt_result(trusted_before: bool, prompt_result: bool) -> bool {
    trusted_before || prompt_result
}

#[cfg(target_os = "macos")]
pub fn focused_window_title(pid: u32) -> Option<String> {
    platform::focused_window_title(pid)
}

#[cfg(target_os = "macos")]
pub fn window_titles(pid: u32) -> Vec<String> {
    platform::window_titles(pid)
}

#[cfg(target_os = "macos")]
pub fn rows(pid: u32) -> Vec<Row> {
    platform::rows(pid)
}

/// Synthetic left click at the centre of `rect`. Returns whether the events
/// could be posted (the caller must verify the result via AX read-back).
#[cfg(target_os = "macos")]
pub fn click(rect: Rect) -> bool {
    platform::click(rect)
}

// ── Non-macOS stubs: the crate still compiles/links, jumps degrade honestly ──

#[cfg(not(target_os = "macos"))]
pub fn is_trusted() -> bool {
    false
}

#[cfg(not(target_os = "macos"))]
pub fn focused_window_title(_pid: u32) -> Option<String> {
    None
}

#[cfg(not(target_os = "macos"))]
pub fn window_titles(_pid: u32) -> Vec<String> {
    Vec::new()
}

#[cfg(not(target_os = "macos"))]
pub fn rows(_pid: u32) -> Vec<Row> {
    Vec::new()
}

#[cfg(not(target_os = "macos"))]
pub fn click(_rect: Rect) -> bool {
    false
}

// ── macOS implementation ────────────────────────────────────────

#[cfg(target_os = "macos")]
mod platform {
    use super::{FocusedConversation, Rect, Row, TextRun};
    use std::ffi::{c_char, c_void, CStr, CString};
    use std::time::Duration;

    type CfType = *const c_void;
    type CfString = *const c_void;
    type CfArray = *const c_void;
    type CfAllocator = *const c_void;
    type AxElement = *const c_void;

    /// UTF-8 `CFStringEncoding`.
    const CF_STRING_ENCODING_UTF8: u32 = 0x0800_0100;
    /// `kAXValueCGPointType` / `kAXValueCGSizeType`.
    const AX_VALUE_CG_POINT: u32 = 1;
    const AX_VALUE_CG_SIZE: u32 = 2;
    /// `kCGEvent*` / `kCGHIDEventTap` / `kCGMouseButtonLeft`.
    const CG_EVENT_LEFT_MOUSE_DOWN: u32 = 1;
    const CG_EVENT_LEFT_MOUSE_UP: u32 = 2;
    const CG_EVENT_MOUSE_MOVED: u32 = 5;
    const CG_HID_EVENT_TAP: u32 = 0;
    const CG_MOUSE_BUTTON_LEFT: u32 = 0;
    const CG_EVENT_SOURCE_STATE_HID_SYSTEM: i32 = 1;
    /// Traversal guard rails: Electron AX trees are large but finite.
    const MAX_DEPTH: usize = 80;
    const MAX_NODES: usize = 80_000;

    #[repr(C)]
    #[derive(Clone, Copy, Debug, Default)]
    pub struct CGPoint {
        pub x: f64,
        pub y: f64,
    }

    #[repr(C)]
    #[derive(Clone, Copy, Debug, Default)]
    pub struct CGSize {
        pub width: f64,
        pub height: f64,
    }

    #[link(name = "ApplicationServices", kind = "framework")]
    extern "C" {
        fn AXIsProcessTrusted() -> u8;
        /// Shows the macOS "add this app to Accessibility" prompt when the
        /// options dictionary contains `AXTrustedCheckOptionPrompt = true`.
        fn AXIsProcessTrustedWithOptions(options: CfType) -> u8;
        fn AXUIElementCreateApplication(pid: i32) -> AxElement;
        fn AXUIElementCopyAttributeValue(
            element: AxElement,
            attribute: CfString,
            value: *mut CfType,
        ) -> i32;
        fn AXValueGetValue(value: CfType, the_type: u32, value_ptr: *mut c_void) -> u8;
        fn AXValueGetTypeID() -> usize;
    }

    #[link(name = "CoreFoundation", kind = "framework")]
    extern "C" {
        fn CFRelease(cf: CfType);
        fn CFRetain(cf: CfType) -> CfType;
        fn CFGetTypeID(cf: CfType) -> usize;
        fn CFStringGetTypeID() -> usize;
        fn CFArrayGetTypeID() -> usize;
        fn CFURLGetTypeID() -> usize;
        fn CFURLGetString(url: CfType) -> CfString;
        fn CFStringCreateWithCString(
            alloc: CfAllocator,
            c_str: *const c_char,
            encoding: u32,
        ) -> CfString;
        fn CFStringGetLength(string: CfString) -> isize;
        fn CFStringGetMaximumSizeForEncoding(length: isize, encoding: u32) -> isize;
        fn CFStringGetCString(
            string: CfString,
            buffer: *mut c_char,
            buffer_size: isize,
            encoding: u32,
        ) -> u8;
        fn CFArrayGetCount(array: CfArray) -> isize;
        fn CFArrayGetValueAtIndex(array: CfArray, index: isize) -> *const c_void;
        /// Key/value callbacks with CFString equality + CFRetain/CFRelease
        /// semantics; required for AXIsProcessTrustedWithOptions to find its key.
        static kCFTypeDictionaryKeyCallBacks: CFDictionaryCallBacks;
        static kCFTypeDictionaryValueCallBacks: CFDictionaryCallBacks;
        static kCFBooleanTrue: *const c_void;
        fn CFDictionaryCreate(
            alloc: CfAllocator,
            keys: *const CfType,
            values: *const CfType,
            num_values: isize,
            key_callbacks: *const c_void,
            value_callbacks: *const c_void,
        ) -> CfType;
    }

    /// Opaque `CFDictionaryKeyCallBacks`/`CFDictionaryValueCallBacks` storage;
    /// only passed by pointer to `CFDictionaryCreate`.
    #[repr(C)]
    struct CFDictionaryCallBacks {
        _private: [u8; 0],
    }

    #[link(name = "CoreGraphics", kind = "framework")]
    extern "C" {
        fn CGEventCreate(source: CfType) -> CfType;
        fn CGEventSourceCreate(state_id: i32) -> CfType;
        fn CGEventCreateMouseEvent(
            source: CfType,
            mouse_type: u32,
            position: CGPoint,
            mouse_button: u32,
        ) -> CfType;
        fn CGEventPost(tap: u32, event: CfType);
        fn CGEventGetLocation(event: CfType) -> CGPoint;
        fn CGWarpMouseCursorPosition(position: CGPoint) -> i32;
    }

    /// Owning CF wrapper: released exactly once on drop.
    struct Cf {
        ptr: CfType,
    }

    impl Cf {
        fn new(ptr: CfType) -> Option<Self> {
            (!ptr.is_null()).then_some(Self { ptr })
        }

        fn as_ptr(&self) -> CfType {
            self.ptr
        }
    }

    impl Drop for Cf {
        fn drop(&mut self) {
            // Safety: `ptr` is always a +1 reference owned by this wrapper.
            unsafe { CFRelease(self.ptr) };
        }
    }

    /// Borrowed array item, retained so it survives the array's release.
    fn retain(item: CfType) -> Option<Cf> {
        if item.is_null() {
            return None;
        }
        // Safety: CFRetain returns a +1 reference for a valid CF object.
        let retained = unsafe { CFRetain(item) };
        Cf::new(retained)
    }

    fn cf_string(value: &str) -> Option<Cf> {
        let c_string = CString::new(value).ok()?;
        // Safety: valid C string, UTF-8 encoding constant.
        let ptr = unsafe {
            CFStringCreateWithCString(std::ptr::null(), c_string.as_ptr(), CF_STRING_ENCODING_UTF8)
        };
        Cf::new(ptr)
    }

    fn type_id_matches(cf: &Cf, type_id: usize) -> bool {
        // Safety: CFGetTypeID accepts any non-null CF object.
        unsafe { CFGetTypeID(cf.as_ptr()) == type_id }
    }

    fn string_value(cf: &Cf) -> Option<String> {
        // Safety: type-checked CFString before the FFI calls.
        if !type_id_matches(cf, unsafe { CFStringGetTypeID() }) {
            return None;
        }
        // Safety: all calls below operate on the checked CFString.
        unsafe {
            let length = CFStringGetLength(cf.as_ptr());
            let capacity =
                CFStringGetMaximumSizeForEncoding(length, CF_STRING_ENCODING_UTF8) + 1;
            if capacity <= 1 {
                return Some(String::new());
            }
            let mut buffer = vec![0u8; capacity as usize];
            let ok = CFStringGetCString(
                cf.as_ptr(),
                buffer.as_mut_ptr() as *mut c_char,
                capacity,
                CF_STRING_ENCODING_UTF8,
            );
            if ok == 0 {
                return None;
            }
            let end = buffer.iter().position(|&b| b == 0).unwrap_or(buffer.len());
            Some(String::from_utf8_lossy(&buffer[..end]).into_owned())
        }
    }

    fn array_items(cf: &Cf) -> Vec<Cf> {
        if !type_id_matches(cf, unsafe { CFArrayGetTypeID() }) {
            return Vec::new();
        }
        // Safety: type-checked CFArray; items are retained before the array dies.
        unsafe {
            let count = CFArrayGetCount(cf.as_ptr());
            let mut out = Vec::with_capacity(count.max(0) as usize);
            for index in 0..count {
                let item = CFArrayGetValueAtIndex(cf.as_ptr(), index);
                if let Some(item) = retain(item) {
                    out.push(item);
                }
            }
            out
        }
    }

    fn app_element(pid: u32) -> Option<Cf> {
        // Safety: AXUIElementCreateApplication returns +1 for a live pid.
        Cf::new(unsafe { AXUIElementCreateApplication(pid as i32) })
    }

    fn copy_attribute(element: AxElement, name: &str) -> Option<Cf> {
        let attribute = cf_string(name)?;
        let mut value: CfType = std::ptr::null();
        // Safety: element is a valid AXUIElement; out param receives +1 or null.
        let error = unsafe {
            AXUIElementCopyAttributeValue(element, attribute.as_ptr(), &mut value)
        };
        if error != 0 {
            return None;
        }
        Cf::new(value)
    }

    fn attribute_string(element: AxElement, name: &str) -> Option<String> {
        copy_attribute(element, name).and_then(|value| string_value(&value))
    }

    fn children(element: AxElement) -> Vec<Cf> {
        copy_attribute(element, "AXChildren")
            .map(|value| array_items(&value))
            .unwrap_or_default()
    }

    fn element_rect(element: AxElement) -> Option<Rect> {
        let position = copy_attribute(element, "AXPosition")?;
        let size = copy_attribute(element, "AXSize")?;
        let ax_value_type_id = unsafe { AXValueGetTypeID() };
        if !type_id_matches(&position, ax_value_type_id) || !type_id_matches(&size, ax_value_type_id)
        {
            return None;
        }
        let mut point = CGPoint::default();
        let mut extent = CGSize::default();
        // Safety: both values were type-checked as AXValue; CGPoint/CGSize match
        // the requested AXValueType.
        let read_point = unsafe {
            AXValueGetValue(
                position.as_ptr(),
                AX_VALUE_CG_POINT,
                &mut point as *mut CGPoint as *mut c_void,
            )
        };
        let read_size = unsafe {
            AXValueGetValue(
                size.as_ptr(),
                AX_VALUE_CG_SIZE,
                &mut extent as *mut CGSize as *mut c_void,
            )
        };
        if read_point == 0 || read_size == 0 {
            return None;
        }
        let rect = Rect {
            x: point.x,
            y: point.y,
            w: extent.width,
            h: extent.height,
        };
        rect.is_sane().then_some(rect)
    }

    /// Visible text of a text-ish element. Chromium uses `AXValue` for static
    /// text, AppKit uses `AXTitle`, and some elements expose only a description.
    fn element_text(element: AxElement) -> Option<String> {
        for attribute in ["AXValue", "AXTitle", "AXDescription"] {
            if let Some(value) = attribute_string(element, attribute) {
                let trimmed = value.trim();
                if !trimmed.is_empty() {
                    return Some(trimmed.to_string());
                }
            }
        }
        None
    }

    fn is_text_role(role: &str) -> bool {
        matches!(
            role,
            "AXStaticText" | "AXTextField" | "AXHeading" | "AXLink" | "AXCell"
        )
    }

    fn is_container_role(role: &str) -> bool {
        // Only outline/table containers: Chromium also exposes chat content as
        // AXList rows (image grids), which must not pollute the sidebar rows.
        matches!(role, "AXOutline" | "AXTable")
    }

    fn collect_texts(element: AxElement, depth: usize, budget: &mut usize, out: &mut Vec<TextRun>) {
        if *budget == 0 || depth > MAX_DEPTH {
            return;
        }
        *budget -= 1;
        let role = attribute_string(element, "AXRole").unwrap_or_default();
        if is_text_role(&role) {
            if let Some(text) = element_text(element) {
                let already = out
                    .last()
                    .map(|previous| previous.text == text)
                    .unwrap_or(false);
                if !already {
                    out.push(TextRun {
                        text,
                        frame: element_rect(element),
                    });
                }
            }
        }
        for child in children(element) {
            collect_texts(child.as_ptr(), depth + 1, budget, out);
        }
    }

    fn collect_rows(element: AxElement, depth: usize, budget: &mut usize, out: &mut Vec<Row>) {
        if *budget == 0 || depth > MAX_DEPTH {
            return;
        }
        *budget -= 1;
        let role = attribute_string(element, "AXRole").unwrap_or_default();
        if role == "AXRow" {
            let frame = element_rect(element);
            let mut texts = Vec::new();
            collect_texts(element, depth + 1, budget, &mut texts);
            out.push(Row { texts, frame });
            return;
        }
        for child in children(element) {
            collect_rows(child.as_ptr(), depth + 1, budget, out);
        }
    }

    /// Walk the window in document order until the first outline/table
    /// container is found, then collect only its rows. The sidebar outline
    /// comes before chat content in the AX tree; later outlines (e.g. chat
    /// image grids) must not pollute the session list.
    ///
    /// Returns whether a container was found.
    fn collect_first_container_rows(
        element: AxElement,
        depth: usize,
        budget: &mut usize,
        out: &mut Vec<Row>,
    ) -> bool {
        if *budget == 0 || depth > MAX_DEPTH {
            return false;
        }
        *budget -= 1;
        let role = attribute_string(element, "AXRole").unwrap_or_default();
        if is_container_role(&role) {
            collect_rows(element, depth + 1, budget, out);
            return true;
        }
        for child in children(element) {
            if collect_first_container_rows(child.as_ptr(), depth + 1, budget, out) {
                return true;
            }
        }
        false
    }

    fn windows(app: AxElement) -> Vec<Cf> {
        copy_attribute(app, "AXWindows")
            .map(|value| array_items(&value))
            .unwrap_or_default()
    }

    fn references(element: AxElement, out: &mut Vec<String>) {
        for name in ["AXIdentifier", "AXDOMIdentifier", "AXDocument", "AXURL"] {
            if let Some(value) = copy_attribute(element, name) {
                let text = if type_id_matches(&value, unsafe { CFURLGetTypeID() }) {
                    retain(unsafe { CFURLGetString(value.as_ptr()) }).and_then(|s| string_value(&s))
                } else { string_value(&value) };
                if let Some(text) = text { out.push(text); }
            }
        }
    }

    fn collect_selected(element: AxElement, depth: usize, budget: &mut usize, selected: bool, out: &mut FocusedConversation) {
        if depth > MAX_DEPTH || *budget == 0 { return; }
        *budget -= 1;
        let role = attribute_string(element, "AXRole").unwrap_or_default();
        let selected = selected || (matches!(role.as_str(), "AXRow" | "AXTab" | "AXRadioButton")
            && copy_attribute(element, "AXSelected").map(|v| v.as_ptr() == unsafe { kCFBooleanTrue }).unwrap_or(false));
        if selected {
            references(element, &mut out.references);
            if let Some(text) = element_text(element) { out.selected_titles.push(text); }
        }
        for child in children(element) {
            collect_selected(child.as_ptr(), depth + 1, budget, selected, out);
        }
    }

    #[link(name = "objc")]
    extern "C" {
        fn objc_getClass(name: *const c_char) -> *mut c_void;
        fn sel_registerName(name: *const c_char) -> *mut c_void;
        fn objc_msgSend();
        fn objc_autoreleasePoolPush() -> *mut c_void;
        fn objc_autoreleasePoolPop(pool: *mut c_void);
    }

    #[link(name = "AppKit", kind = "framework")]
    extern "C" {}

    /// NSWorkspace supplies the real foreground PID and bundle identity.
    /// AX application titles are optional and are not application identities.
    pub(super) fn foreground_application() -> Option<(i32, String, String)> {
        struct Pool(*mut c_void);
        impl Drop for Pool {
            fn drop(&mut self) { unsafe { objc_autoreleasePoolPop(self.0) }; }
        }
        unsafe fn message(object: *mut c_void, name: &[u8]) -> *mut c_void {
            if object.is_null() { return std::ptr::null_mut(); }
            let send: unsafe extern "C" fn(*mut c_void, *mut c_void) -> *mut c_void =
                std::mem::transmute(objc_msgSend as *const ());
            send(object, sel_registerName(name.as_ptr().cast()))
        }
        unsafe fn string(object: *mut c_void) -> Option<String> {
            let utf8 = message(object, b"UTF8String\0") as *const c_char;
            (!utf8.is_null()).then(|| CStr::from_ptr(utf8).to_string_lossy().into_owned())
        }
        // Calls below use AppKit's documented zero-argument selectors. Objects
        // are borrowed inside one autorelease pool; strings are copied out.
        unsafe {
            let _pool = Pool(objc_autoreleasePoolPush());
            let workspace = message(objc_getClass(b"NSWorkspace\0".as_ptr().cast()), b"sharedWorkspace\0");
            let app = message(workspace, b"frontmostApplication\0");
            if app.is_null() { return None; }
            let send_pid: unsafe extern "C" fn(*mut c_void, *mut c_void) -> i32 =
                std::mem::transmute(objc_msgSend as *const ());
            let pid = send_pid(app, sel_registerName(b"processIdentifier\0".as_ptr().cast()));
            let name = string(message(app, b"localizedName\0"))?;
            let bundle = string(message(app, b"bundleIdentifier\0"))?;
            (pid > 0).then_some((pid, name, bundle))
        }
    }

    pub fn focused_conversation(bundle_ids: &[String]) -> Option<FocusedConversation> {
        if !is_trusted() { return None; }
        let (pid, app_name, bundle_id) = foreground_application()?;
        if !bundle_ids.iter().any(|id| id == &bundle_id) { return None; }
        let app = app_element(pid as u32)?;
        let window = copy_attribute(app.as_ptr(), "AXFocusedWindow")?;
        let mut out = FocusedConversation {
            app_name,
            bundle_id,
            title: attribute_string(window.as_ptr(), "AXTitle").unwrap_or_default(),
            ..Default::default()
        };
        references(window.as_ptr(), &mut out.references);
        // A VSCode chat input lives inside a webview whose URL identifies the
        // active session. Read the focused branch, never background chat tabs.
        if let Some(mut focused) = copy_attribute(app.as_ptr(), "AXFocusedUIElement") {
            for _ in 0..MAX_DEPTH {
                references(focused.as_ptr(), &mut out.references);
                if out.focused_web_title.is_none() && attribute_string(focused.as_ptr(), "AXRole").as_deref() == Some("AXWebArea") {
                    out.focused_web_title = attribute_string(focused.as_ptr(), "AXTitle").filter(|s| !s.trim().is_empty());
                }
                let Some(parent) = copy_attribute(focused.as_ptr(), "AXParent") else { break; };
                focused = parent;
            }
        }
        let mut budget = 2_000;
        collect_selected(window.as_ptr(), 0, &mut budget, false, &mut out);
        Some(out)
    }

    pub fn is_trusted() -> bool {
        // Safety: documented, side-effect-free system query.
        unsafe { AXIsProcessTrusted() != 0 }
    }

    /// Show the native Accessibility prompt for the CURRENT executable and
    /// return whether the process is trusted at that moment (usually still
    /// `false`: the grant happens later in System Settings).
    pub fn prompt_accessibility_permission() -> Result<bool, String> {
        let key = cf_string("AXTrustedCheckOptionPrompt")
            .ok_or_else(|| "cf_string_failed".to_string())?;
        // Safety: kCFBooleanTrue is a constant CFBoolean; the dictionary retains
        // it via kCFTypeDictionaryValueCallBacks. Keys/values arrays live for the
        // whole call.
        let keys: [CfType; 1] = [key.as_ptr()];
        let values: [CfType; 1] = [unsafe { kCFBooleanTrue }];
        let dictionary = unsafe {
            CFDictionaryCreate(
                std::ptr::null(),
                keys.as_ptr(),
                values.as_ptr(),
                1,
                &kCFTypeDictionaryKeyCallBacks as *const CFDictionaryCallBacks as *const c_void,
                &kCFTypeDictionaryValueCallBacks as *const CFDictionaryCallBacks as *const c_void,
            )
        };
        let Some(dictionary) = Cf::new(dictionary) else {
            return Err("cf_dictionary_failed".to_string());
        };
        // Safety: valid CFDictionary; returns a Boolean (unsigned char).
        let trusted = unsafe { AXIsProcessTrustedWithOptions(dictionary.as_ptr()) != 0 };
        Ok(trusted)
    }

    pub fn focused_window_title(pid: u32) -> Option<String> {
        let app = app_element(pid)?;
        if let Some(window) = copy_attribute(app.as_ptr(), "AXFocusedWindow") {
            if let Some(title) = attribute_string(window.as_ptr(), "AXTitle") {
                if !title.trim().is_empty() {
                    return Some(title.trim().to_string());
                }
            }
        }
        windows(app.as_ptr())
            .iter()
            .find_map(|window| attribute_string(window.as_ptr(), "AXTitle"))
            .map(|title| title.trim().to_string())
            .filter(|title| !title.is_empty())
    }

    pub fn window_titles(pid: u32) -> Vec<String> {
        let Some(app) = app_element(pid) else {
            return Vec::new();
        };
        windows(app.as_ptr())
            .iter()
            .filter_map(|window| attribute_string(window.as_ptr(), "AXTitle"))
            .map(|title| title.trim().to_string())
            .filter(|title| !title.is_empty())
            .collect()
    }

    pub fn rows(pid: u32) -> Vec<Row> {
        let Some(app) = app_element(pid) else {
            return Vec::new();
        };
        for window in windows(app.as_ptr()) {
            let mut out = Vec::new();
            let mut budget = MAX_NODES;
            if collect_first_container_rows(window.as_ptr(), 0, &mut budget, &mut out) {
                return out;
            }
        }
        Vec::new()
    }

    pub fn click(rect: Rect) -> bool {
        if !rect.is_sane() || !is_trusted() {
            return false;
        }
        let (center_x, center_y) = rect.center();
        let point = CGPoint {
            x: center_x,
            y: center_y,
        };
        let saved_cursor = cursor_position();
        // Safety: all CG calls below accept NULL sources/events and return
        // null on failure, which is checked; every created object is released.
        unsafe {
            CGWarpMouseCursorPosition(point);
            let source = CGEventSourceCreate(CG_EVENT_SOURCE_STATE_HID_SYSTEM);
            let mut posted = true;
            for mouse_type in [
                CG_EVENT_MOUSE_MOVED,
                CG_EVENT_LEFT_MOUSE_DOWN,
                CG_EVENT_LEFT_MOUSE_UP,
            ] {
                let event = CGEventCreateMouseEvent(source, mouse_type, point, CG_MOUSE_BUTTON_LEFT);
                if event.is_null() {
                    posted = false;
                    break;
                }
                CGEventPost(CG_HID_EVENT_TAP, event);
                CFRelease(event);
                std::thread::sleep(Duration::from_millis(40));
            }
            if !source.is_null() {
                CFRelease(source);
            }
            if let Some(saved) = saved_cursor {
                // Never leave the user's pointer parked on the clicked row.
                CGWarpMouseCursorPosition(saved);
            }
            posted
        }
    }

    fn cursor_position() -> Option<CGPoint> {
        // Safety: CGEventCreate(NULL) returns the current event state or null.
        unsafe {
            let event = CGEventCreate(std::ptr::null());
            if event.is_null() {
                return None;
            }
            let location = CGEventGetLocation(event);
            CFRelease(event);
            Some(location)
        }
    }

}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rect_center_and_sanity_guard_the_click_path() {
        let rect = Rect {
            x: 100.0,
            y: 200.0,
            w: 40.0,
            h: 20.0,
        };
        assert_eq!(rect.center(), (120.0, 210.0));
        assert!(rect.is_sane());
        assert!(!Rect::default().is_sane());
        assert!(!Rect {
            x: f64::NAN,
            y: 0.0,
            w: 10.0,
            h: 10.0
        }
        .is_sane());
        assert!(!Rect {
            x: 0.0,
            y: 0.0,
            w: 0.5,
            h: 10.0
        }
        .is_sane());
    }

    #[test]
    fn row_texts_are_trimmed_and_filtered() {
        let row = Row {
            texts: vec![
                TextRun {
                    text: " 进行中 ".into(),
                    frame: None,
                },
                TextRun {
                    text: "   ".into(),
                    frame: None,
                },
                TextRun {
                    text: "多窗口询问提醒与跳转".into(),
                    frame: None,
                },
            ],
            frame: None,
        };
        assert_eq!(row.text_strings(), vec!["进行中", "多窗口询问提醒与跳转"]);
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn permission_prompt_result_never_regresses_a_trusted_process() {
        // Already trusted: no prompt is shown and the result stays true.
        assert!(resolve_prompt_result(true, false));
        assert!(resolve_prompt_result(true, true));
        // Freshly trusted via the prompt.
        assert!(resolve_prompt_result(false, true));
        // Prompt shown but the grant has not happened yet.
        assert!(!resolve_prompt_result(false, false));
    }

    #[test]
    fn row_serialises_texts_and_frames_for_the_debug_cli() {
        let row = Row {
            texts: vec![TextRun {
                text: "q".into(),
                frame: Some(Rect {
                    x: 1.0,
                    y: 2.0,
                    w: 3.0,
                    h: 4.0,
                }),
            }],
            frame: Some(Rect {
                x: 5.0,
                y: 6.0,
                w: 7.0,
                h: 8.0,
            }),
        };
        let json = serde_json::to_value(&row).unwrap();
        assert_eq!(json["texts"][0]["text"], "q");
        assert_eq!(json["texts"][0]["frame"]["x"], 1.0);
        assert_eq!(json["frame"]["w"], 7.0);
    }
}
