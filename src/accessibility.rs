//! The accessibility settings both embedders share, and the screen reader they
//! start.
//!
//! The application and the greeter each keep their own saved choices (the
//! user's `settings.json`, the greeter's state file). Under both sits one
//! system-wide file, [`DEFAULTS_PATH`], written by the desicompass NixOS module
//! or by hand on any other distribution. A value is resolved as: what the
//! embedder saved, else this file, else the embedder's built-in default.
//!
//! The keys are the application's own `settings.json` keys, so the same
//! spelling means the same thing in all three places.

use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};

use serde_json::{Map, Value};

use crate::app_state::{AppRenderer, PaletteTheme};

/// The system-wide defaults file.
pub const DEFAULTS_PATH: &str = "/etc/sicompass/accessibility.json";

pub const KEY_SCREEN_READER: &str = "screenReader";
pub const KEY_FONT_SCALE: &str = "fontScale";
pub const KEY_COLOR_SCHEME: &str = "colorScheme";
pub const KEY_LANGUAGE: &str = "language";
pub const KEY_SHOULDER_SURFING: &str = "shoulderSurfingProtection";

/// The font-scale radio's options, as stored.
pub const FONT_SCALES: &[&str] = &["1.00", "1.25", "1.50", "1.75", "2.00", "2.25", "2.50"];
/// The colour-scheme radio's options, as stored.
pub const COLOR_SCHEMES: &[&str] = &["dark", "light"];
/// The locales the language radio offers. Anything else is ignored, so a stale
/// value can never lock the interface to a bundle that does not exist.
pub const LANGUAGES: &[&str] = &["en-US", "nl-BE", "fr-BE", "de-BE"];

/// The screen reader started when nobody names another one.
pub const DEFAULT_SCREEN_READER: &str = "orca";

/// A set of accessibility choices, any of which may be unset.
///
/// Used both for the system defaults and for an embedder's own saved values,
/// which is what lets [`or`](Self::or) layer one over the other.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct AccessibilitySettings {
    pub screen_reader: Option<bool>,
    /// One of [`FONT_SCALES`].
    pub font_scale: Option<String>,
    /// One of [`COLOR_SCHEMES`].
    pub color_scheme: Option<String>,
    /// One of [`LANGUAGES`].
    pub language: Option<String>,
    pub shoulder_surfing_protection: Option<bool>,
}

impl AccessibilitySettings {
    /// The system defaults, from [`DEFAULTS_PATH`]. Empty off Linux, where
    /// nothing writes that file.
    pub fn system() -> Self {
        if cfg!(target_os = "linux") {
            Self::load(Path::new(DEFAULTS_PATH))
        } else {
            Self::default()
        }
    }

    /// Read a flat JSON object of accessibility keys. Never fails: a missing or
    /// unreadable file, or one that is not an object, gives an empty set, and a
    /// value that is out of range is dropped rather than guessed at.
    pub fn load(path: &Path) -> Self {
        std::fs::read_to_string(path)
            .ok()
            .and_then(|s| serde_json::from_str::<Value>(&s).ok())
            .and_then(|v| v.as_object().map(Self::from_object))
            .unwrap_or_default()
    }

    /// Pick the accessibility keys out of an object. Other keys are ignored,
    /// so this also reads the `sicompass` section of the app's `settings.json`.
    pub fn from_object(obj: &Map<String, Value>) -> Self {
        Self {
            screen_reader: obj.get(KEY_SCREEN_READER).and_then(parse_bool),
            font_scale: obj.get(KEY_FONT_SCALE).and_then(parse_font_scale),
            color_scheme: obj
                .get(KEY_COLOR_SCHEME)
                .and_then(Value::as_str)
                .filter(|s| COLOR_SCHEMES.contains(s))
                .map(str::to_owned),
            language: obj
                .get(KEY_LANGUAGE)
                .and_then(Value::as_str)
                .filter(|s| LANGUAGES.contains(s))
                .map(str::to_owned),
            shoulder_surfing_protection: obj.get(KEY_SHOULDER_SURFING).and_then(parse_bool),
        }
    }

    /// The set values as a flat object, in the same shape [`load`](Self::load)
    /// reads.
    pub fn to_object(&self) -> Map<String, Value> {
        let mut m = Map::new();
        if let Some(b) = self.screen_reader {
            m.insert(KEY_SCREEN_READER.to_owned(), Value::Bool(b));
        }
        if let Some(s) = &self.font_scale {
            m.insert(KEY_FONT_SCALE.to_owned(), Value::String(s.clone()));
        }
        if let Some(s) = &self.color_scheme {
            m.insert(KEY_COLOR_SCHEME.to_owned(), Value::String(s.clone()));
        }
        if let Some(s) = &self.language {
            m.insert(KEY_LANGUAGE.to_owned(), Value::String(s.clone()));
        }
        if let Some(b) = self.shoulder_surfing_protection {
            m.insert(KEY_SHOULDER_SURFING.to_owned(), Value::Bool(b));
        }
        m
    }

    /// Each field from `self` if set, otherwise from `lower`.
    pub fn or(self, lower: Self) -> Self {
        Self {
            screen_reader: self.screen_reader.or(lower.screen_reader),
            font_scale: self.font_scale.or(lower.font_scale),
            color_scheme: self.color_scheme.or(lower.color_scheme),
            language: self.language.or(lower.language),
            shoulder_surfing_protection: self
                .shoulder_surfing_protection
                .or(lower.shoulder_surfing_protection),
        }
    }

    /// Set one key from its stored string form, as a settings row reports it.
    /// Returns false for an unknown key or an invalid value, leaving `self`
    /// unchanged.
    pub fn set(&mut self, key: &str, value: &str) -> bool {
        let v = Value::String(value.to_owned());
        let mut one = Map::new();
        one.insert(key.to_owned(), v);
        let parsed = Self::from_object(&one);
        if parsed == Self::default() {
            return false;
        }
        *self = parsed.or(std::mem::take(self));
        true
    }
}

/// A boolean written either as JSON `true`/`false` or as the string form a
/// checkbox row reports.
fn parse_bool(v: &Value) -> Option<bool> {
    match v {
        Value::Bool(b) => Some(*b),
        Value::String(s) if s == "true" => Some(true),
        Value::String(s) if s == "false" => Some(false),
        _ => None,
    }
}

/// A font scale written as `"1.75"` or `1.75`, clamped to the offered range and
/// snapped to the nearest option.
fn parse_font_scale(v: &Value) -> Option<String> {
    let f = match v {
        Value::String(s) => s.trim().parse::<f32>().ok()?,
        Value::Number(n) => n.as_f64()? as f32,
        _ => return None,
    };
    if !f.is_finite() {
        return None;
    }
    let snapped = ((f.clamp(1.0, 2.5) * 4.0).round()) / 4.0;
    Some(format!("{snapped:.2}"))
}

/// The numeric scale for a stored font-scale string, or the renderer default.
pub fn font_scale_value(stored: Option<&str>) -> f32 {
    stored
        .and_then(|s| s.parse::<f32>().ok())
        .filter(|f| f.is_finite())
        .map(|f| f.clamp(1.0, 2.5))
        .unwrap_or(crate::registry::DEFAULT_FONT_SCALE)
}

/// Apply a display setting to the renderer. Returns true if `key` was one of
/// them (`colorScheme`, `shoulderSurfingProtection`, `fontScale`).
///
/// A font-scale change only flags the rebuild: the renderer then asks
/// [`HostHooks::read_font_scale`](crate::registry::HostHooks::read_font_scale)
/// for the new value, so the embedder must have stored it first.
pub fn apply_display(renderer: &mut AppRenderer, key: &str, value: &str) -> bool {
    match key {
        KEY_COLOR_SCHEME => {
            renderer.palette_theme = if value == "light" {
                PaletteTheme::Light
            } else {
                PaletteTheme::Dark
            };
            true
        }
        KEY_SHOULDER_SURFING => {
            renderer.privacy_blank = value == "true";
            true
        }
        KEY_FONT_SCALE => {
            renderer.rebuild_font_renderer = true;
            true
        }
        _ => false,
    }
}

/// A screen reader this process started, and stops again.
///
/// Dropping it stops the screen reader. That matters for the greeter: Orca has
/// to be gone before greetd hands the display to the user's session, which may
/// start its own.
pub struct ScreenReader {
    command: PathBuf,
    args: Vec<String>,
    child: Option<Child>,
}

impl ScreenReader {
    /// Orca, or whatever `command` names, started with `--replace` so that a
    /// screen reader already on this bus hands over rather than both speaking.
    pub fn new(command: impl Into<PathBuf>) -> Self {
        Self::with_args(command, vec!["--replace".to_owned()])
    }

    pub fn with_args(command: impl Into<PathBuf>, args: Vec<String>) -> Self {
        Self {
            command: command.into(),
            args,
            child: None,
        }
    }

    /// True while the process this started is still running.
    pub fn is_running(&mut self) -> bool {
        match self.child.as_mut().map(Child::try_wait) {
            Some(Ok(None)) => true,
            Some(_) => {
                // Exited (or cannot be asked): forget it so `start` can retry.
                self.child = None;
                false
            }
            None => false,
        }
    }

    /// Start it, unless it is already running.
    pub fn start(&mut self) -> std::io::Result<()> {
        if self.is_running() {
            return Ok(());
        }
        let child = Command::new(&self.command)
            .args(&self.args)
            .stdin(Stdio::null())
            .spawn()?;
        tracing::info!(
            "started screen reader {:?} (pid {})",
            self.command,
            child.id()
        );
        self.child = Some(child);
        Ok(())
    }

    /// Stop it, if this started it. Waits for the process so it leaves no zombie.
    pub fn stop(&mut self) {
        if let Some(mut child) = self.child.take() {
            let _ = child.kill();
            let _ = child.wait();
            tracing::info!("stopped screen reader {:?}", self.command);
        }
    }
}

/// Orca, not started.
impl Default for ScreenReader {
    fn default() -> Self {
        Self::new(DEFAULT_SCREEN_READER)
    }
}

impl Drop for ScreenReader {
    fn drop(&mut self) {
        self.stop();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    fn write(dir: &tempfile::TempDir, text: &str) -> PathBuf {
        let p = dir.path().join("accessibility.json");
        std::fs::File::create(&p)
            .unwrap()
            .write_all(text.as_bytes())
            .unwrap();
        p
    }

    #[test]
    fn a_missing_file_gives_no_opinions() {
        let dir = tempfile::tempdir().unwrap();
        let s = AccessibilitySettings::load(&dir.path().join("nope.json"));
        assert_eq!(s, AccessibilitySettings::default());
    }

    #[test]
    fn a_corrupt_file_gives_no_opinions() {
        let dir = tempfile::tempdir().unwrap();
        assert_eq!(
            AccessibilitySettings::load(&write(&dir, "{not json")),
            AccessibilitySettings::default()
        );
        assert_eq!(
            AccessibilitySettings::load(&write(&dir, "[1, 2]")),
            AccessibilitySettings::default()
        );
    }

    #[test]
    fn a_full_file_is_read() {
        let dir = tempfile::tempdir().unwrap();
        let p = write(
            &dir,
            r#"{"screenReader": true, "fontScale": "2.00", "colorScheme": "light",
                "language": "nl-BE", "shoulderSurfingProtection": false}"#,
        );
        assert_eq!(
            AccessibilitySettings::load(&p),
            AccessibilitySettings {
                screen_reader: Some(true),
                font_scale: Some("2.00".to_owned()),
                color_scheme: Some("light".to_owned()),
                language: Some("nl-BE".to_owned()),
                shoulder_surfing_protection: Some(false),
            }
        );
    }

    #[test]
    fn a_partial_file_leaves_the_rest_unset() {
        let dir = tempfile::tempdir().unwrap();
        let s = AccessibilitySettings::load(&write(&dir, r#"{"screenReader": "true"}"#));
        assert_eq!(s.screen_reader, Some(true));
        assert_eq!(s.font_scale, None);
        assert_eq!(s.language, None);
    }

    #[test]
    fn out_of_range_values_are_clamped_or_dropped() {
        let dir = tempfile::tempdir().unwrap();
        let s = AccessibilitySettings::load(&write(
            &dir,
            r#"{"fontScale": 9, "colorScheme": "purple", "language": "xx-XX",
                "screenReader": "yes"}"#,
        ));
        assert_eq!(s.font_scale.as_deref(), Some("2.50"));
        assert_eq!(s.color_scheme, None);
        assert_eq!(s.language, None);
        assert_eq!(s.screen_reader, None);

        let s = AccessibilitySettings::load(&write(&dir, r#"{"fontScale": "1.3"}"#));
        assert_eq!(s.font_scale.as_deref(), Some("1.25"));
        let s = AccessibilitySettings::load(&write(&dir, r#"{"fontScale": "0.2"}"#));
        assert_eq!(s.font_scale.as_deref(), Some("1.00"));
    }

    #[test]
    fn or_prefers_the_upper_layer_field_by_field() {
        let upper = AccessibilitySettings {
            screen_reader: Some(false),
            ..Default::default()
        };
        let lower = AccessibilitySettings {
            screen_reader: Some(true),
            language: Some("fr-BE".to_owned()),
            ..Default::default()
        };
        let s = upper.or(lower);
        assert_eq!(s.screen_reader, Some(false));
        assert_eq!(s.language.as_deref(), Some("fr-BE"));
    }

    #[test]
    fn set_accepts_valid_values_and_refuses_the_rest() {
        let mut s = AccessibilitySettings {
            language: Some("de-BE".to_owned()),
            ..Default::default()
        };
        assert!(s.set(KEY_SCREEN_READER, "false"));
        assert_eq!(s.screen_reader, Some(false));
        assert_eq!(s.language.as_deref(), Some("de-BE"), "other keys survive");
        assert!(!s.set(KEY_LANGUAGE, "xx-XX"));
        assert_eq!(s.language.as_deref(), Some("de-BE"));
        assert!(!s.set("somethingElse", "true"));
    }

    #[test]
    fn to_object_round_trips() {
        let s = AccessibilitySettings {
            screen_reader: Some(true),
            font_scale: Some("1.50".to_owned()),
            color_scheme: None,
            language: Some("en-US".to_owned()),
            shoulder_surfing_protection: Some(true),
        };
        assert_eq!(AccessibilitySettings::from_object(&s.to_object()), s);
    }

    #[test]
    fn font_scale_value_falls_back_to_the_default() {
        assert_eq!(font_scale_value(Some("2.00")), 2.0);
        assert_eq!(
            font_scale_value(Some("junk")),
            crate::registry::DEFAULT_FONT_SCALE
        );
        assert_eq!(font_scale_value(None), crate::registry::DEFAULT_FONT_SCALE);
    }

    #[test]
    fn apply_display_sets_the_renderer() {
        let mut r = AppRenderer::new();
        assert!(apply_display(&mut r, KEY_COLOR_SCHEME, "light"));
        assert_eq!(r.palette_theme, PaletteTheme::Light);
        assert!(apply_display(&mut r, KEY_COLOR_SCHEME, "dark"));
        assert_eq!(r.palette_theme, PaletteTheme::Dark);

        assert!(apply_display(&mut r, KEY_SHOULDER_SURFING, "true"));
        assert!(r.privacy_blank);
        assert!(apply_display(&mut r, KEY_SHOULDER_SURFING, "false"));
        assert!(!r.privacy_blank);

        r.rebuild_font_renderer = false;
        assert!(apply_display(&mut r, KEY_FONT_SCALE, "2.00"));
        assert!(r.rebuild_font_renderer);

        assert!(!apply_display(&mut r, KEY_LANGUAGE, "nl-BE"));
        assert!(!apply_display(&mut r, KEY_SCREEN_READER, "true"));
    }

    fn sleeper() -> ScreenReader {
        ScreenReader::with_args("sleep", vec!["30".to_owned()])
    }

    #[test]
    fn a_screen_reader_starts_once_and_stops() {
        let mut sr = sleeper();
        assert!(!sr.is_running());
        sr.start().unwrap();
        assert!(sr.is_running());
        let pid = sr.child.as_ref().unwrap().id();
        sr.start().unwrap();
        assert_eq!(
            sr.child.as_ref().unwrap().id(),
            pid,
            "a second start must not spawn a second screen reader"
        );
        sr.stop();
        assert!(!sr.is_running());
    }

    #[test]
    fn dropping_a_screen_reader_stops_it() {
        let mut sr = sleeper();
        sr.start().unwrap();
        let pid = sr.child.as_ref().unwrap().id();
        drop(sr);
        // Killed and reaped by `stop`, so the process is gone entirely.
        assert!(!Path::new(&format!("/proc/{pid}")).exists());
    }

    #[test]
    fn a_screen_reader_that_exits_can_be_started_again() {
        let mut sr = ScreenReader::with_args("true", Vec::new());
        sr.start().unwrap();
        // `true` exits at once; wait for it.
        for _ in 0..200 {
            if !sr.is_running() {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        assert!(!sr.is_running());
        sr.start().unwrap();
    }

    #[test]
    fn a_missing_screen_reader_is_an_error_not_a_panic() {
        let mut sr = ScreenReader::new("/nonexistent/orca");
        assert!(sr.start().is_err());
        assert!(!sr.is_running());
    }
}
