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

impl Provider for Launcher {
    fn name(&self) -> &str {
        "launcher"
    }
    fn fetch(&mut self) -> Vec<FfonElement> {
        match self.path.as_str() {
            "/Windows" => windows(),
            "/Controls" => controls(),
            _ => {
                let mut w = FfonElement::new_obj("Windows");
                for c in windows() {
                    w.as_obj_mut().unwrap().push(c);
                }
                let mut c = FfonElement::new_obj("Controls");
                for e in controls() {
                    c.as_obj_mut().unwrap().push(e);
                }
                vec![w, c, button("app:foot", "Foot"), button("app:bash", "Bash")]
            }
        }
    }
    fn push_path(&mut self, seg: &str) {
        self.path = format!("/{seg}");
    }
    fn pop_path(&mut self) {
        self.path = "/".to_owned();
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
        ["+ Windows", "+ Controls", "-b Foot", "-b Bash"]
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
    assert_eq!(r.current_id, id(&[0, 3]), "the cursor stays on the button");
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
