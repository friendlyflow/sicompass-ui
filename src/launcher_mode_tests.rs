//! Launcher mode (`AppRenderer::launcher_mode`), driven through the same entry
//! points the event loop uses: `shortcuts::dispatch_key` for keys and
//! `handlers::handle_input` for typed text.

use std::sync::{Arc, Mutex};

use sdl3::keyboard::{Keycode, Mod};
use sicompass_sdk::ffon::{FfonElement, IdArray};
use sicompass_sdk::provider::Provider;

use crate::app_state::{AppRenderer, Coordinate};
use crate::{handlers, registry, shortcuts};

/// Path-scoped, like a real launcher: the root holds two sections and two
/// programs, each section holds buttons.
struct Launcher {
    path: String,
    pressed: Arc<Mutex<Vec<String>>>,
}

fn button(f: &str, label: &str) -> FfonElement {
    FfonElement::Str(format!("<button>{f}</button>{label}"))
}

fn windows() -> Vec<FfonElement> {
    vec![button("window:1", "foot"), button("window:2", "firefox")]
}

fn controls() -> Vec<FfonElement> {
    vec![button("power:reboot", "Reboot")]
}

fn settings() -> Vec<FfonElement> {
    let mut scheme = FfonElement::new_obj("<radio>color scheme");
    let o = scheme.as_obj_mut().unwrap();
    o.push(FfonElement::Str(sicompass_sdk::tags::format_checked(
        "dark",
    )));
    o.push(FfonElement::Str("light".to_owned()));
    vec![
        FfonElement::Str(sicompass_sdk::tags::format_checkbox("screen reader")),
        scheme,
    ]
}

impl Provider for Launcher {
    fn name(&self) -> &str {
        "launcher"
    }
    fn fetch(&mut self) -> Vec<FfonElement> {
        match self.path.as_str() {
            "/Windows" => windows(),
            "/Controls" => controls(),
            "/Settings" => settings(),
            "/Settings/color scheme" => settings()[1].as_obj().unwrap().children.clone(),
            _ => {
                let mut w = FfonElement::new_obj("Windows");
                for c in windows() {
                    w.as_obj_mut().unwrap().push(c);
                }
                let mut c = FfonElement::new_obj("Controls");
                for e in controls() {
                    c.as_obj_mut().unwrap().push(e);
                }
                let mut st = FfonElement::new_obj("Settings");
                for e in settings() {
                    st.as_obj_mut().unwrap().push(e);
                }
                vec![
                    w,
                    c,
                    st,
                    button("app:foot", "Foot"),
                    button("app:bash", "Bash"),
                ]
            }
        }
    }
    fn push_path(&mut self, seg: &str) {
        let seg = sicompass_sdk::tags::strip_display(seg);
        self.path = if self.path == "/" {
            format!("/{seg}")
        } else {
            format!("{}/{seg}", self.path)
        };
    }
    fn pop_path(&mut self) {
        match self.path.rfind('/') {
            Some(0) | None => self.path = "/".to_owned(),
            Some(i) => self.path.truncate(i),
        }
    }
    fn set_current_path(&mut self, path: &str) {
        self.path = path.to_owned();
    }
    fn current_path(&self) -> &str {
        &self.path
    }
    fn on_button_press(&mut self, function_name: &str) {
        self.pressed.lock().unwrap().push(function_name.to_owned());
    }
    fn on_checkbox_change(&mut self, label: &str, checked: bool) {
        let label = sicompass_sdk::tags::strip_display(label);
        self.pressed
            .lock()
            .unwrap()
            .push(format!("checkbox:{label}:{checked}"));
    }
    fn on_radio_change(&mut self, group: &str, value: &str) {
        let group = sicompass_sdk::tags::strip_display(group);
        self.pressed
            .lock()
            .unwrap()
            .push(format!("radio:{group}:{value}"));
    }
}

fn id(parts: &[usize]) -> IdArray {
    let mut a = IdArray::new();
    for &p in parts {
        a.push(p);
    }
    a
}

fn launcher() -> (AppRenderer, Arc<Mutex<Vec<String>>>) {
    let pressed = Arc::new(Mutex::new(Vec::new()));
    let mut r = AppRenderer::new();
    registry::register_provider(
        &mut r,
        Box::new(Launcher {
            path: "/".to_owned(),
            pressed: Arc::clone(&pressed),
        }),
    );
    r.launcher_mode = true;
    handlers::open_in_search(&mut r, &id(&[0, 0]));
    (r, pressed)
}

fn key(r: &mut AppRenderer, k: Keycode) {
    shortcuts::dispatch_key(r, Some(k), Mod::empty());
}

fn visible_labels(r: &AppRenderer) -> Vec<String> {
    if r.filtered_list_indices.is_empty() && r.search_string.is_empty() {
        r.total_list.iter().map(|i| i.label.clone()).collect()
    } else {
        r.filtered_list_indices
            .iter()
            .map(|&i| r.total_list[i].label.clone())
            .collect()
    }
}

fn path(r: &AppRenderer) -> String {
    crate::provider::current_path(r).to_owned()
}

#[test]
fn off_by_default() {
    let r = AppRenderer::new();
    assert!(!r.launcher_mode);
    assert!(!r.launcher_window);
    assert!(!r.suspended);
    assert!(!r.dismiss_requested);
}

#[test]
fn opening_lands_in_simple_search_at_the_root() {
    let (r, _) = launcher();
    assert_eq!(r.coordinate, Coordinate::SimpleSearch);
    assert_eq!(r.current_id, id(&[0, 0]));
    assert_eq!(path(&r), "/");
    assert_eq!(
        visible_labels(&r),
        [
            "+ Windows",
            "+ Controls",
            "+ Settings",
            "-b Foot",
            "-b Bash"
        ]
    );
}

#[test]
fn opening_on_a_section_lands_inside_it() {
    let (mut r, _) = launcher();
    handlers::open_in_search(&mut r, &id(&[0, 0, 1]));
    assert_eq!(r.coordinate, Coordinate::SimpleSearch);
    assert_eq!(r.current_id, id(&[0, 0, 1]));
    assert_eq!(path(&r), "/Windows");
    assert_eq!(visible_labels(&r), ["-b foot", "-b firefox"]);
}

#[test]
fn opening_again_forgets_the_previous_query_and_level() {
    let (mut r, _) = launcher();
    handlers::open_in_search(&mut r, &id(&[0, 1, 0]));
    handlers::handle_input(&mut r, "reb");
    handlers::open_in_search(&mut r, &id(&[0, 0]));
    assert!(r.search_string.is_empty());
    assert_eq!(path(&r), "/");
    assert_eq!(r.current_id, id(&[0, 0]));
}

#[test]
fn a_row_past_the_end_is_clamped() {
    let (mut r, _) = launcher();
    handlers::open_in_search(&mut r, &id(&[0, 1, 9]));
    assert_eq!(r.current_id, id(&[0, 1, 0]));
}

#[test]
fn enter_on_a_section_enters_it_and_keeps_searching() {
    let (mut r, pressed) = launcher();
    handlers::handle_input(&mut r, "contr");
    key(&mut r, Keycode::Return);
    assert_eq!(r.coordinate, Coordinate::SimpleSearch);
    assert!(r.search_string.is_empty());
    assert_eq!(path(&r), "/Controls");
    assert_eq!(visible_labels(&r), ["-b Reboot"]);
    assert!(pressed.lock().unwrap().is_empty());
}

#[test]
fn enter_on_a_button_presses_it_at_once() {
    let (mut r, pressed) = launcher();
    handlers::handle_input(&mut r, "bash");
    key(&mut r, Keycode::Return);
    assert_eq!(*pressed.lock().unwrap(), ["app:bash"]);
    assert_eq!(r.coordinate, Coordinate::SimpleSearch);
    assert!(r.search_string.is_empty(), "the query is cleared");
    assert_eq!(r.current_id, id(&[0, 4]), "the cursor stays on the button");
}

#[test]
fn enter_with_nothing_matching_does_nothing() {
    let (mut r, pressed) = launcher();
    handlers::handle_input(&mut r, "zzzz");
    key(&mut r, Keycode::Return);
    assert!(pressed.lock().unwrap().is_empty());
    assert_eq!(r.coordinate, Coordinate::SimpleSearch);
}

#[test]
fn escape_asks_to_be_dismissed_and_changes_nothing_else() {
    let (mut r, _) = launcher();
    handlers::handle_input(&mut r, "fo");
    key(&mut r, Keycode::Escape);
    assert!(r.dismiss_requested);
    assert_eq!(r.coordinate, Coordinate::SimpleSearch);
    assert_eq!(r.search_string, "fo");
}

#[test]
fn left_never_reaches_the_provider_list() {
    let (mut r, _) = launcher();
    key(&mut r, Keycode::Left);
    assert_eq!(r.current_id.depth(), 2);
    assert_eq!(r.coordinate, Coordinate::SimpleSearch);

    handlers::open_in_search(&mut r, &id(&[0, 0, 0]));
    key(&mut r, Keycode::Left);
    assert_eq!(r.current_id.depth(), 2, "one level up, back to the root");
    assert_eq!(path(&r), "/");
}

#[test]
fn keys_that_would_leave_search_are_ignored() {
    let (mut r, _) = launcher();
    key(&mut r, Keycode::Tab);
    shortcuts::dispatch_key(&mut r, Some(Keycode::F), Mod::LCTRLMOD);
    shortcuts::dispatch_key(&mut r, Some(Keycode::T), Mod::LCTRLMOD);
    shortcuts::dispatch_key(&mut r, Some(Keycode::Return), Mod::LCTRLMOD);
    assert_eq!(r.coordinate, Coordinate::SimpleSearch);
    assert_eq!(r.current_id, id(&[0, 0]));
}

#[test]
fn up_and_down_move_through_the_list() {
    let (mut r, _) = launcher();
    key(&mut r, Keycode::Down);
    assert_eq!(r.current_list_item_id(), Some(id(&[0, 1])));
    key(&mut r, Keycode::Up);
    assert_eq!(r.current_list_item_id(), Some(id(&[0, 0])));
}

#[test]
fn the_row_prefix_is_not_searched_in_launcher_mode() {
    let (mut r, _) = launcher();
    handlers::handle_input(&mut r, "b");
    // Only "Bash" has a b of its own; "-b Foot" used to match on its prefix.
    assert_eq!(visible_labels(&r), ["-b Bash"]);
    // The highlight sits on "B" in the full label "-b Bash", after "-b ".
    assert_eq!(r.fuzzy_match_positions, vec![vec![3]]);
}

#[test]
fn outside_launcher_mode_the_prefix_is_still_searched() {
    let (mut r, _) = launcher();
    r.launcher_mode = false;
    handlers::handle_input(&mut r, "b");
    assert_eq!(visible_labels(&r).len(), 2, "both -b rows match, as before");
}

#[test]
fn outside_launcher_mode_escape_still_leaves_search() {
    let (mut r, _) = launcher();
    r.launcher_mode = false;
    key(&mut r, Keycode::Escape);
    assert!(!r.dismiss_requested);
    assert_ne!(r.coordinate, Coordinate::SimpleSearch);
}

#[test]
fn enter_on_a_checkbox_toggles_it_and_keeps_searching() {
    let (mut r, pressed) = launcher();
    handlers::open_in_search(&mut r, &id(&[0, 2, 0]));
    handlers::handle_input(&mut r, "screen");
    key(&mut r, Keycode::Return);
    assert_eq!(*pressed.lock().unwrap(), ["checkbox:screen reader:true"]);
    assert_eq!(r.coordinate, Coordinate::SimpleSearch);
    assert!(r.search_string.is_empty());
    assert_eq!(
        r.current_id,
        id(&[0, 2, 0]),
        "the cursor stays on the checkbox"
    );
}

#[test]
fn enter_on_a_radio_group_opens_it_and_enter_on_an_option_chooses_it() {
    let (mut r, pressed) = launcher();
    handlers::open_in_search(&mut r, &id(&[0, 2, 1]));
    key(&mut r, Keycode::Return);
    assert_eq!(visible_labels(&r), ["-rc dark", "-r light"]);
    handlers::handle_input(&mut r, "light");
    key(&mut r, Keycode::Return);
    assert_eq!(*pressed.lock().unwrap(), ["radio:color scheme:light"]);
    assert_eq!(r.coordinate, Coordinate::SimpleSearch);
    assert!(r.search_string.is_empty());
}

#[test]
fn a_plain_row_is_still_not_an_action() {
    let (mut r, pressed) = launcher();
    r.ffon[0]
        .as_obj_mut()
        .unwrap()
        .push(FfonElement::Str("just text".into()));
    handlers::handle_input(&mut r, "just");
    key(&mut r, Keycode::Return);
    assert!(pressed.lock().unwrap().is_empty());
}

/// The superkey inside its tutorial: a launcher's window, in General mode with
/// the app's keymap.
fn launcher_window_in_general() -> AppRenderer {
    let (mut r, _) = launcher();
    r.launcher_window = true;
    r.launcher_mode = false;
    key(&mut r, Keycode::Escape); // out of search, the app's way
    assert_eq!(r.coordinate, Coordinate::General);
    assert!(!r.dismiss_requested, "Escape in search only leaves search");
    r
}

fn ctrl(r: &mut AppRenderer, k: Keycode, shift: bool) {
    let m = if shift {
        Mod::LCTRLMOD | Mod::LSHIFTMOD
    } else {
        Mod::LCTRLMOD
    };
    shortcuts::dispatch_key(r, Some(k), m);
}

#[test]
fn escape_in_general_closes_a_launchers_window() {
    let mut r = launcher_window_in_general();
    key(&mut r, Keycode::Escape);
    assert!(r.dismiss_requested);
}

#[test]
fn escape_in_general_does_not_close_the_app() {
    let mut r = launcher_window_in_general();
    r.launcher_window = false;
    key(&mut r, Keycode::Escape);
    assert!(!r.dismiss_requested);
}

#[test]
fn a_launchers_window_has_no_tabs() {
    let mut r = launcher_window_in_general();
    ctrl(&mut r, Keycode::T, false);
    assert_eq!(r.tabs.len(), 1, "Ctrl+T");
    key(&mut r, Keycode::T);
    assert_eq!(r.coordinate, Coordinate::General, "t opens no switcher");

    // Even with a second tab somehow there, nothing reaches it.
    r.launcher_window = false;
    ctrl(&mut r, Keycode::T, false);
    assert_eq!(r.tabs.len(), 2, "the app has tabs");
    r.launcher_window = true;
    let active = r.active_tab;
    ctrl(&mut r, Keycode::T, true);
    assert_eq!(r.tabs.len(), 2, "Ctrl+Shift+T");
    ctrl(&mut r, Keycode::Tab, false);
    assert_eq!(r.coordinate, Coordinate::General, "Ctrl+Tab");
    for k in [Keycode::_1, Keycode::_2, Keycode::_9] {
        ctrl(&mut r, k, false);
        assert_eq!(r.active_tab, active, "Ctrl+{k:?}");
    }
}

#[test]
fn a_launchers_window_hints_escape_as_close_and_no_tab_keys() {
    let r = launcher_window_in_general();
    let hints = shortcuts::hints_for(&r);
    let has = |h: &str| hints.iter().any(|x| x.starts_with(h));
    assert!(has("Esc    Close"), "{hints:?}");
    assert!(!has("Esc    Back"), "{hints:?}");
    assert!(!has("Ctrl+T"), "{hints:?}");
    assert!(!has("t      "), "{hints:?}");
}

#[test]
fn a_launchers_window_has_no_undo_redo_or_timeline() {
    let mut r = launcher_window_in_general();
    let before = r.ffon.clone();
    ctrl(&mut r, Keycode::Z, false);
    ctrl(&mut r, Keycode::Z, true);
    assert_eq!(r.ffon, before);
    assert_eq!(r.coordinate, Coordinate::General);
    key(&mut r, Keycode::Z);
    assert_eq!(r.coordinate, Coordinate::General, "z opens no timeline");

    let hints = shortcuts::hints_for(&r);
    for h in ["Ctrl+Z", "Ctrl+Shift+Z", "Z      "] {
        assert!(!hints.iter().any(|x| x.starts_with(h)), "{h}: {hints:?}");
    }
}

#[test]
fn the_app_keeps_its_timeline() {
    let mut r = launcher_window_in_general();
    r.launcher_window = false;
    key(&mut r, Keycode::Z);
    assert_eq!(r.coordinate, Coordinate::TimelineView);
}
