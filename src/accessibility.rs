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
/// [`stop`](Self::stop) asks it to quit, so Orca can say "screen reader off"
/// first. Dropping it stops the screen reader at once and quietly. That matters
/// for the greeter: Orca has to be gone before greetd hands the display to the
/// user's session, which may start its own.
pub struct ScreenReader {
    command: PathBuf,
    args: Vec<String>,
    child: Option<Child>,
    /// How long [`stop`](Self::stop) lets it take before killing it. Orca gives
    /// itself five seconds to shut down (a `SIGALRM` in `orca.shutdown`).
    grace: std::time::Duration,
    /// Whether the one started last is listening yet. See
    /// [`take_ready`](Self::take_ready).
    watch: Option<std::sync::Arc<ReadyWatch>>,
}

/// Shared with the thread that waits for a started screen reader to listen.
#[derive(Default)]
struct ReadyWatch {
    ready: std::sync::atomic::AtomicBool,
    /// Set when it is stopped or replaced, so a watcher for a screen reader
    /// that is gone gives up.
    cancelled: std::sync::atomic::AtomicBool,
}

/// A socket the screen reader reports "ready" on, in the `sd_notify` protocol.
///
/// Orca speaks that protocol to systemd when `NOTIFY_SOCKET` is set: it sends
/// `READY=1` at the very end of its startup, once it listens for events and has
/// looked for the focused window. Handing it a socket of ours gets exactly that
/// moment. (An earlier version guessed from Orca's D-Bus name plus a margin;
/// at a cold boot Orca was still loading its scripts when the margin ran out.)
///
/// Abstract (Linux), so there is no file to clean up. A screen reader other
/// than Orca ignores the variable, and then simply never reports ready.
#[cfg(target_os = "linux")]
fn notify_socket() -> std::io::Result<(std::os::unix::net::UnixDatagram, String)> {
    use std::os::linux::net::SocketAddrExt;
    use std::os::unix::net::{SocketAddr, UnixDatagram};
    use std::sync::atomic::{AtomicU32, Ordering};

    static NEXT: AtomicU32 = AtomicU32::new(0);
    let name = format!(
        "sicompass-screen-reader-{}-{}",
        std::process::id(),
        NEXT.fetch_add(1, Ordering::Relaxed)
    );
    let socket = UnixDatagram::bind_addr(&SocketAddr::from_abstract_name(name.as_bytes())?)?;
    // So the thread can notice a cancellation while waiting.
    socket.set_read_timeout(Some(std::time::Duration::from_millis(250)))?;
    Ok((socket, format!("@{name}")))
}

/// Wait on `socket` for `READY=1`, then set `ready`. Gives up when the watch is
/// cancelled, or after a minute.
#[cfg(target_os = "linux")]
fn wait_for_ready(socket: &std::os::unix::net::UnixDatagram, pid: u32, watch: &ReadyWatch) {
    use std::sync::atomic::Ordering;

    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(60);
    let mut buf = [0u8; 4096];
    while std::time::Instant::now() < deadline {
        if watch.cancelled.load(Ordering::Acquire) {
            return;
        }
        let Ok(n) = socket.recv(&mut buf) else {
            continue; // the read timeout, most likely
        };
        // One datagram may carry several newline-separated assignments.
        if buf[..n]
            .split(|b| *b == b'\n')
            .any(|line| line == b"READY=1")
        {
            if !watch.cancelled.load(Ordering::Acquire) {
                tracing::info!("the screen reader (pid {pid}) is ready");
                watch.ready.store(true, Ordering::Release);
            }
            return;
        }
    }
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
            grace: std::time::Duration::from_secs(6),
            watch: None,
        }
    }

    /// True exactly once: when the screen reader started last has begun
    /// listening for accessibility events.
    ///
    /// Orca looks for the focused window once, while it starts. At the login
    /// screen the window is often not registered yet, so it finds nothing, and
    /// the focus event the window sends on registering arrives before Orca
    /// listens. It then says nothing until a key is pressed. The embedder
    /// answers this with a window focus toggle (`AppRenderer::a11y_refocus_now`),
    /// which Orca, ready by then, answers by reading the focused row.
    ///
    /// Orca reports ready over the socket named in `NOTIFY_SOCKET` (see
    /// [`notify_socket`]). Linux only.
    pub fn take_ready(&mut self) -> bool {
        self.watch
            .as_ref()
            .is_some_and(|w| w.ready.swap(false, std::sync::atomic::Ordering::AcqRel))
    }

    /// Stop waiting for the previous one, if any.
    fn cancel_watch(&mut self) {
        if let Some(old) = self.watch.take() {
            old.cancelled
                .store(true, std::sync::atomic::Ordering::Release);
        }
    }

    /// Wait, on a thread, for the screen reader with this pid to report ready
    /// on `socket`.
    #[cfg(target_os = "linux")]
    fn watch_for_ready(&mut self, pid: u32, socket: std::os::unix::net::UnixDatagram) {
        self.cancel_watch();
        let watch = std::sync::Arc::new(ReadyWatch::default());
        self.watch = Some(std::sync::Arc::clone(&watch));
        let _ = std::thread::Builder::new()
            .name("screen-reader-ready".into())
            .spawn(move || wait_for_ready(&socket, pid, &watch));
    }

    /// Change how long [`stop`](Self::stop) waits before killing it.
    pub fn with_grace(mut self, grace: std::time::Duration) -> Self {
        self.grace = grace;
        self
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

    /// How it ended, if it has exited without being stopped. Returned once;
    /// after that it counts as not running, so `start` can run it again.
    ///
    /// For the embedder to log: a screen reader that quits by itself leaves a
    /// blind user with nothing, and the exit status (a signal, or a code) is
    /// the only trace of why.
    pub fn take_unexpected_exit(&mut self) -> Option<std::process::ExitStatus> {
        let status = self.child.as_mut()?.try_wait().ok()??;
        self.child = None;
        Some(status)
    }

    /// Start it, unless it is already running.
    pub fn start(&mut self) -> std::io::Result<()> {
        if self.is_running() {
            return Ok(());
        }
        let mut cmd = Command::new(&self.command);
        cmd.args(&self.args).stdin(Stdio::null());
        #[cfg(unix)]
        {
            use std::os::unix::process::CommandExt;
            // Its own session, so it has no controlling terminal and is in no
            // process group but its own. At the login screen everything else
            // shares the console greetd opened, and a signal meant for that
            // console or group (SIGINT, SIGTERM) makes Orca shut down, saying
            // "screen reader off".
            // SAFETY: `setsid` runs between fork and exec, is async-signal-safe
            // and touches no memory of ours. It can only fail for a process
            // group leader, which a freshly forked child never is.
            unsafe {
                cmd.pre_exec(|| {
                    libc::setsid();
                    Ok(())
                });
            }
        }
        #[cfg(target_os = "linux")]
        let notify = match notify_socket() {
            Ok((socket, name)) => {
                cmd.env("NOTIFY_SOCKET", name);
                Some(socket)
            }
            Err(e) => {
                tracing::warn!("no ready socket for the screen reader: {e}");
                None
            }
        };
        let child = cmd.spawn()?;
        tracing::info!(
            "started screen reader {:?} (pid {})",
            self.command,
            child.id()
        );
        self.cancel_watch();
        #[cfg(target_os = "linux")]
        if let Some(socket) = notify {
            self.watch_for_ready(child.id(), socket);
        }
        self.child = Some(child);
        Ok(())
    }

    /// Ask it to quit, if this started it, and return at once.
    ///
    /// `SIGTERM`, not `SIGKILL`: Orca answers it by saying "screen reader off"
    /// and shutting down, and killing it outright left a user who had just
    /// unticked the box in silence, not knowing whether it had worked. A thread
    /// waits for it to exit (so it leaves no zombie) and kills it if it is still
    /// there after the grace period. Elsewhere than Unix it is killed at once.
    pub fn stop(&mut self) {
        self.cancel_watch();
        let Some(mut child) = self.child.take() else {
            return;
        };
        let command = self.command.clone();
        #[cfg(unix)]
        {
            // SAFETY: `kill` takes a pid and a signal number and touches no
            // memory of ours. The pid is our own unreaped child, so it cannot
            // have been reused for another process.
            let pid = child.id() as libc::pid_t;
            if unsafe { libc::kill(pid, libc::SIGTERM) } == 0 {
                let grace = self.grace;
                // If the thread cannot be spawned the child goes with it,
                // already told to quit, and is only left unreaped.
                let _ = std::thread::Builder::new()
                    .name("screen-reader-stop".into())
                    .spawn(move || {
                        let deadline = std::time::Instant::now() + grace;
                        while std::time::Instant::now() < deadline {
                            if let Ok(Some(_)) = child.try_wait() {
                                tracing::info!("stopped screen reader {command:?}");
                                return;
                            }
                            std::thread::sleep(std::time::Duration::from_millis(50));
                        }
                        tracing::warn!("screen reader {command:?} ignored SIGTERM; killing it");
                        let _ = child.kill();
                        let _ = child.wait();
                    });
                return;
            }
        }
        let _ = child.kill();
        let _ = child.wait();
        tracing::info!("stopped screen reader {command:?}");
    }

    /// Stop it at once, without letting it speak. For shutting down.
    pub fn stop_now(&mut self) {
        self.cancel_watch();
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
        self.stop_now();
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

    fn gone_within(pid: u32, limit: std::time::Duration) -> bool {
        let deadline = std::time::Instant::now() + limit;
        while std::time::Instant::now() < deadline {
            if !Path::new(&format!("/proc/{pid}")).exists() {
                return true;
            }
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
        false
    }

    /// `stop` asks rather than kills, so Orca can say "screen reader off".
    /// `sleep` quits on SIGTERM, and the reaper thread then collects it.
    #[test]
    fn stop_asks_it_to_quit_and_it_is_reaped() {
        let mut sr = sleeper().with_grace(std::time::Duration::from_secs(10));
        sr.start().unwrap();
        let pid = sr.child.as_ref().unwrap().id();
        let before = std::time::Instant::now();
        sr.stop();
        assert!(
            before.elapsed() < std::time::Duration::from_millis(500),
            "stop must not block the render loop"
        );
        assert!(!sr.is_running());
        assert!(
            gone_within(pid, std::time::Duration::from_secs(3)),
            "SIGTERM must end it well inside the grace period"
        );
    }

    /// A screen reader that hangs on the way out is killed after the grace.
    #[test]
    fn one_that_ignores_sigterm_is_killed_after_the_grace() {
        // An ignored signal stays ignored across exec, so `sleep` itself (same
        // pid) ignores SIGTERM.
        let mut sr = ScreenReader::with_args(
            "sh",
            vec!["-c".to_owned(), "trap '' TERM; exec sleep 30".to_owned()],
        )
        .with_grace(std::time::Duration::from_millis(300));
        sr.start().unwrap();
        let pid = sr.child.as_ref().unwrap().id();
        // Let the shell reach the exec, so the trap is in place.
        std::thread::sleep(std::time::Duration::from_millis(200));
        sr.stop();
        assert!(
            gone_within(pid, std::time::Duration::from_secs(5)),
            "it must be killed once the grace runs out"
        );
    }

    /// A screen reader that never says it is ready is never ready.
    #[test]
    fn a_screen_reader_that_never_reports_is_never_ready() {
        let mut sr = sleeper();
        sr.start().unwrap();
        std::thread::sleep(std::time::Duration::from_millis(600));
        assert!(!sr.take_ready());
        sr.stop_now();
    }

    /// The whole path: the socket named in `NOTIFY_SOCKET` reaches the screen
    /// reader, and `READY=1` sent there is reported once.
    #[cfg(target_os = "linux")]
    #[test]
    fn ready_sent_on_the_notify_socket_is_reported() {
        use std::os::linux::net::SocketAddrExt;
        use std::os::unix::net::{SocketAddr, UnixDatagram};

        let dir = tempfile::tempdir().unwrap();
        let out = dir.path().join("notify-socket");
        let mut sr = ScreenReader::with_args(
            "sh",
            vec![
                "-c".to_owned(),
                format!(
                    "printf %s \"$NOTIFY_SOCKET\" > {}; exec sleep 30",
                    out.display()
                ),
            ],
        );
        sr.start().unwrap();

        let mut name = String::new();
        for _ in 0..200 {
            name = std::fs::read_to_string(&out).unwrap_or_default();
            if !name.is_empty() {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        let abstract_name = name.strip_prefix('@').expect("an abstract socket name");
        let tx = UnixDatagram::unbound().unwrap();
        let addr = SocketAddr::from_abstract_name(abstract_name.as_bytes()).unwrap();
        // Several assignments in one datagram, as sd_notify allows.
        tx.send_to_addr(b"STATUS=starting\nREADY=1", &addr).unwrap();

        let mut ready = false;
        for _ in 0..200 {
            if sr.take_ready() {
                ready = true;
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        assert!(ready, "READY=1 must be reported");
        assert!(!sr.take_ready(), "once");
        sr.stop_now();
    }

    #[test]
    fn readiness_is_reported_once_and_forgotten_on_stop() {
        let mut sr = sleeper();
        sr.start().unwrap();
        let watch = sr.watch.clone().expect("start watches");
        watch
            .ready
            .store(true, std::sync::atomic::Ordering::Release);
        assert!(sr.take_ready());
        assert!(!sr.take_ready(), "once");

        watch
            .ready
            .store(true, std::sync::atomic::Ordering::Release);
        sr.stop_now();
        assert!(watch.cancelled.load(std::sync::atomic::Ordering::Acquire));
        assert!(!sr.take_ready(), "a stopped screen reader is not ready");
    }

    #[test]
    fn a_restart_stops_waiting_for_the_previous_one() {
        let mut sr = sleeper();
        sr.start().unwrap();
        let first = sr.watch.clone().unwrap();
        sr.stop();
        sr.start().unwrap();
        assert!(first.cancelled.load(std::sync::atomic::Ordering::Acquire));
        sr.stop_now();
    }

    #[test]
    fn an_exit_nobody_asked_for_is_reported_once_with_its_status() {
        let mut sr = ScreenReader::with_args("sh", vec!["-c".to_owned(), "exit 3".to_owned()]);
        sr.start().unwrap();
        let mut status = None;
        for _ in 0..200 {
            status = sr.take_unexpected_exit();
            if status.is_some() {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        assert_eq!(status.and_then(|s| s.code()), Some(3));
        assert_eq!(sr.take_unexpected_exit(), None, "reported once");
        assert!(!sr.is_running());
    }

    #[test]
    fn a_running_screen_reader_has_no_exit_to_report() {
        let mut sr = sleeper();
        sr.start().unwrap();
        assert_eq!(sr.take_unexpected_exit(), None);
        sr.stop_now();
    }

    /// Orca must not share the greeter's console or process group.
    #[test]
    fn it_runs_in_a_session_of_its_own() {
        let mut sr = sleeper();
        sr.start().unwrap();
        let pid = sr.child.as_ref().unwrap().id();
        // Field 6 of /proc/<pid>/stat is the session id; after `setsid` it is
        // the process's own pid.
        let mut sid = String::new();
        for _ in 0..100 {
            let stat = std::fs::read_to_string(format!("/proc/{pid}/stat")).unwrap();
            // The command name is in parentheses and may hold spaces.
            let after = &stat[stat.rfind(')').unwrap() + 2..];
            sid = after.split(' ').nth(3).unwrap().to_owned();
            if sid == pid.to_string() {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        sr.stop_now();
        assert_eq!(
            sid,
            pid.to_string(),
            "the screen reader leads its own session"
        );
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
