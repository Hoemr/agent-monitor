//! Custom icon framework: user-supplied images for the collapsed pet and the
//! edge-hidden tab.
//!
//! Storage is deliberately separate from `settings.json`: images live as files
//! in `<data dir>/icons/<slot>.img`, while settings only carries `icon_style`.
//! This keeps the settings file small, makes the payload easy to inspect/remove
//! by hand, and lets the UI fetch the images on demand.
//!
//! Validation happens here (magic-byte sniffing + a hard size limit) so a
//! malformed upload can never become an HTML/executable payload and can never
//! bloat the app data directory.
//!
//! Requirements for authors (also shown in the Settings UI and docs/ICONS.md):
//! square image, recommended 144×144 px (2× of the 72 px default), transparent
//! background, PNG/JPEG/WebP, at most 2 MB.

use base64::Engine;
use serde::Serialize;
use std::path::PathBuf;

pub const MAX_ICON_BYTES: usize = 2 * 1024 * 1024;
pub const SLOTS: [&str; 2] = ["collapsed", "hidden"];

/// Data URLs for both slots, exactly as the frontend consumes them.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize)]
pub struct CustomIcons {
    pub collapsed: Option<String>,
    pub hidden: Option<String>,
}

/// What `save_custom_icon` reports back to the settings page.
#[derive(Clone, Debug, Serialize)]
pub struct IconMeta {
    pub slot: String,
    pub mime: String,
    pub bytes: usize,
    pub updated_at: u64,
}

pub struct IconStore {
    dir: PathBuf,
}

impl IconStore {
    pub fn new(dir: PathBuf) -> Self {
        Self { dir }
    }

    pub fn default_store() -> Self {
        Self::new(crate::settings::data_dir().join("icons"))
    }

    pub fn dir(&self) -> &PathBuf {
        &self.dir
    }

    fn slot_path(&self, slot: &str) -> Result<PathBuf, String> {
        if !SLOTS.contains(&slot) {
            return Err(format!("unknown icon slot: {slot}"));
        }
        Ok(self.dir.join(format!("{slot}.img")))
    }

    /// Decode, validate and atomically store one slot. Returns metadata for the
    /// settings page (mime is sniffed, never trusted from the client).
    pub fn save_data_url(&self, slot: &str, data_url: &str) -> Result<IconMeta, String> {
        let path = self.slot_path(slot)?;
        let payload = data_url
            .split_once(',')
            .map(|(_, payload)| payload)
            .ok_or_else(|| "expected a data URL (data:image/...;base64,...)".to_string())?;
        let bytes = base64::engine::general_purpose::STANDARD
            .decode(payload.trim())
            .map_err(|e| format!("invalid base64 payload: {e}"))?;
        if bytes.len() > MAX_ICON_BYTES {
            return Err(format!(
                "image is too large: {} KB (max {} KB)",
                bytes.len() / 1024,
                MAX_ICON_BYTES / 1024
            ));
        }
        let mime = sniff_mime(&bytes)
            .ok_or_else(|| "unsupported image format (use PNG, JPEG or WebP)".to_string())?;
        std::fs::create_dir_all(&self.dir)
            .map_err(|e| format!("cannot create {}: {e}", self.dir.display()))?;
        let tmp = path.with_extension("img.tmp");
        std::fs::write(&tmp, &bytes).map_err(|e| format!("cannot write {}: {e}", tmp.display()))?;
        std::fs::rename(&tmp, &path).map_err(|e| format!("cannot replace {}: {e}", path.display()))?;
        Ok(IconMeta {
            slot: slot.to_string(),
            mime: mime.to_string(),
            bytes: bytes.len(),
            updated_at: crate::model::now_secs(),
        })
    }

    /// Remove one slot. Removing a slot that does not exist is not an error.
    pub fn clear(&self, slot: &str) -> Result<(), String> {
        let path = self.slot_path(slot)?;
        match std::fs::remove_file(&path) {
            Ok(()) => Ok(()),
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(err) => Err(format!("cannot remove {}: {err}", path.display())),
        }
    }

    pub fn load_all(&self) -> CustomIcons {
        CustomIcons {
            collapsed: self.load_slot("collapsed"),
            hidden: self.load_slot("hidden"),
        }
    }

    /// Metadata of a stored slot without building a data URL.
    pub fn meta(&self, slot: &str) -> Option<IconMeta> {
        let path = self.slot_path(slot).ok()?;
        let bytes = std::fs::read(&path).ok()?;
        if bytes.len() > MAX_ICON_BYTES {
            return None;
        }
        let mime = sniff_mime(&bytes)?;
        let updated_at = std::fs::metadata(&path)
            .and_then(|m| m.modified())
            .map(|t| {
                t.duration_since(std::time::UNIX_EPOCH)
                    .unwrap_or_default()
                    .as_secs()
            })
            .unwrap_or(0);
        Some(IconMeta {
            slot: slot.to_string(),
            mime: mime.to_string(),
            bytes: bytes.len(),
            updated_at,
        })
    }

    fn load_slot(&self, slot: &str) -> Option<String> {
        let path = self.slot_path(slot).ok()?;
        let bytes = std::fs::read(&path).ok()?;
        if bytes.is_empty() || bytes.len() > MAX_ICON_BYTES {
            return None;
        }
        let mime = sniff_mime(&bytes)?;
        let encoded = base64::engine::general_purpose::STANDARD.encode(&bytes);
        Some(format!("data:{mime};base64,{encoded}"))
    }
}

/// Only real image payloads are accepted; the declared mime type is ignored.
pub fn sniff_mime(bytes: &[u8]) -> Option<&'static str> {
    if bytes.starts_with(&[0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A]) {
        return Some("image/png");
    }
    if bytes.starts_with(&[0xFF, 0xD8, 0xFF]) {
        return Some("image/jpeg");
    }
    if bytes.len() >= 12 && &bytes[0..4] == b"RIFF" && &bytes[8..12] == b"WEBP" {
        return Some("image/webp");
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 1×1 transparent PNG.
    const PNG_1PX: &str = "iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR42mNkYPhfDwAChwGA60e6kgAAAABJRU5ErkJggg==";

    fn store(tag: &str) -> (IconStore, PathBuf) {
        let dir = std::env::temp_dir().join(format!("am-icons-{}-{tag}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        (IconStore::new(dir.clone()), dir)
    }

    #[test]
    fn save_load_and_clear_roundtrip() {
        let (s, dir) = store("roundtrip");
        let url = format!("data:image/png;base64,{PNG_1PX}");
        let meta = s.save_data_url("collapsed", &url).expect("save");
        assert_eq!(meta.mime, "image/png");
        assert!(meta.bytes > 0);
        let icons = s.load_all();
        assert_eq!(icons.collapsed.as_deref(), Some(url.as_str()));
        assert!(icons.hidden.is_none());
        assert_eq!(s.meta("collapsed").map(|m| m.mime), Some("image/png".into()));
        s.clear("collapsed").expect("clear");
        assert!(s.load_all().collapsed.is_none());
        // Clearing twice is fine.
        s.clear("collapsed").expect("clear again");
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn rejects_non_image_payloads() {
        let (s, dir) = store("reject");
        let html = base64::engine::general_purpose::STANDARD.encode(b"<html>evil</html>");
        assert!(s.save_data_url("collapsed", &format!("data:text/html;base64,{html}")).is_err());
        assert!(s.save_data_url("collapsed", "not-a-data-url").is_err());
        assert!(s.load_all().collapsed.is_none());
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn rejects_oversized_images_and_unknown_slots() {
        let (s, dir) = store("limits");
        // A real PNG header followed by padding over the limit.
        let mut big = vec![0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A];
        big.resize(MAX_ICON_BYTES + 1, 0);
        let url = format!(
            "data:image/png;base64,{}",
            base64::engine::general_purpose::STANDARD.encode(&big)
        );
        let err = s.save_data_url("collapsed", &url).unwrap_err();
        assert!(err.contains("too large"), "{err}");
        assert!(s.save_data_url("banner", &url).is_err());
        assert!(s.clear("banner").is_err());
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn sniffing_accepts_png_jpeg_webp_only() {
        assert_eq!(sniff_mime(&[0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A]), Some("image/png"));
        assert_eq!(sniff_mime(&[0xFF, 0xD8, 0xFF, 0xE0]), Some("image/jpeg"));
        let mut webp = b"RIFF".to_vec();
        webp.extend_from_slice(&[0, 0, 0, 0]);
        webp.extend_from_slice(b"WEBP");
        assert_eq!(sniff_mime(&webp), Some("image/webp"));
        assert_eq!(sniff_mime(b"GIF89a"), None);
        assert_eq!(sniff_mime(&[]), None);
    }
}
