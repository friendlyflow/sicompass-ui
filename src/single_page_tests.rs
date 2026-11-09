//! A window that shows one provider's page (`AppRenderer::single_page`, the
//! greeter), an embedder's own manual (`AppRenderer::manual`), and a provider
//! refresh while the beginner's manual is open. Driven through the entry points
//! the event loop uses: `shortcuts::dispatch_key` and `view::drain_needs_refresh`.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use sdl3::keyboard::{Keycode, Mod};
use sicompass_sdk::ffon::{FfonElement, IdArray};
use sicompass_sdk::provider::Provider;

use crate::app_state::{AppRenderer, Coordinate, ManualText};
use crate::{handlers, list, registry, shortcuts, view};

/// A login page in miniature: a group of users, a password field, a button, and a
/// clock that asks for a refresh the way the greeter's does.
struct Page {
    path: String,
    refresh: Arc<AtomicBool>,
}

const PASSWORD_ROW: usize = 1;

fn page() -> Vec<FfonElement> {
    let mut users = FfonElement::new_obj("<radio>User");
    let o = users.as_obj_mut().unwrap();
    o.push(FfonElement::Str(sicompass_sdk::tags::format_checked(
        "alice",
    )));
    o.push(FfonElement::Str("bob".to_owned()));
    vec![
        users,
        FfonElement::Str("<input></input>Password:".to_owned()),
        FfonElement::Str("<button>power:off</button>Shut down".to_owned()),
        FfonElement::Str("Monday 9 November, 16:00".to_owned()),
    ]
}

impl Provider for Page {
    fn name(&self) -> &str {
        "page"
    }
    fn display_name(&self) -> String {
        "Sign in".to_owned()
    }
    fn fetch(&mut self) -> Vec<FfonElement> {
        match self.path.as_str() {
            "/User" => page()[0].as_obj().unwrap().children.clone(),
            _ => page(),
        }
    }
    fn push_path(&mut self, seg: &str) {
        let seg = sicompass_sdk::tags::strip_display(seg);
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
    fn needs_refresh(&self) -> bool {
        self.refresh.load(Ordering::SeqCst)
    }
    fn clear_needs_refresh(&mut self) {
        self.refresh.store(false, Ordering::SeqCst);
    }
}

fn id(parts: &[usize]) -> IdArray {
    let mut a = IdArray::new();
    for &p in parts {
        a.push(p);
    }
    a
}

/// The page, opened on its password row, the way the greeter starts.
fn renderer(single_page: bool) -> (AppRenderer, Arc<AtomicBool>) {
    let refresh = Arc::new(AtomicBool::new(false));
    let mut r = AppRenderer::new();
    registry::register_provider(
        &mut r,
        Box::new(Page {
            path: "/".to_owned(),
            refresh: Arc::clone(&refresh),
        }),
    );
    r.single_page = single_page;
    r.current_id = id(&[0, PASSWORD_ROW]);
    list::create_list_current_layer(&mut r);
    r.sync_list_index_from_current_id();
    (r, refresh)
}

fn key(r: &mut AppRenderer, k: Keycode) {
    shortcuts::dispatch_key(r, Some(k), Mod::NOMOD);
}

fn ctrl(r: &mut AppRenderer, k: Keycode) {
    shortcuts::dispatch_key(r, Some(k), Mod::LCTRLMOD);
}

fn rows(r: &AppRenderer) -> Vec<String> {
    r.total_list.iter().map(|i| i.label.clone()).collect()
}

// ---- single_page -----------------------------------------------------------

#[test]
fn a_single_page_has_no_parent_line_and_left_stays_on_it() {
    let (mut r, _) = renderer(true);
    assert_eq!(r.top_depth(), 2);
    assert_eq!(
        list::parent_line_text(&r),
        "",
        "no 'Sign in' line above the page"
    );
    key(&mut r, Keycode::Left);
    assert_eq!(
        r.current_id,
        id(&[0, PASSWORD_ROW]),
        "Left does nothing on the page"
    );
    assert!(
        rows(&r).iter().any(|l| l.contains("Password:")),
        "{:?}",
        rows(&r)
    );
}

#[test]
fn without_single_page_left_climbs_to_the_provider_list() {
    let (mut r, _) = renderer(false);
    assert_eq!(r.top_depth(), 1);
    assert_eq!(list::parent_line_text(&r), "Sign in");
    key(&mut r, Keycode::Left);
    assert_eq!(r.current_id.depth(), 1, "the app keeps its provider list");
}

#[test]
fn inside_a_group_the_parent_line_names_it_and_left_comes_back_to_the_page() {
    let (mut r, _) = renderer(true);
    r.current_id = id(&[0, 0]);
    r.sync_list_index_from_current_id();
    key(&mut r, Keycode::Right);
    assert_eq!(r.current_id.depth(), 3, "Right goes into the user group");
    assert_eq!(list::parent_line_text(&r), "User");
    key(&mut r, Keycode::Left);
    assert_eq!(r.current_id, id(&[0, 0]));
    key(&mut r, Keycode::Left);
    assert_eq!(r.current_id, id(&[0, 0]), "and no further");
}

#[test]
fn double_home_stops_at_the_page() {
    let (mut r, _) = renderer(true);
    r.current_id = id(&[0, 0]);
    r.sync_list_index_from_current_id();
    key(&mut r, Keycode::Right);
    assert_eq!(r.current_id.depth(), 3);
    r.last_keypress_time = handlers::sdl_ticks();
    handlers::handle_home(&mut r);
    assert_eq!(r.current_id.depth(), 2, "Home Home climbs to the page");
    r.last_keypress_time = handlers::sdl_ticks();
    handlers::handle_home(&mut r);
    assert_eq!(
        r.current_id,
        id(&[0, 0]),
        "and stays there, on its first row"
    );
}

#[test]
fn a_double_ctrl_f_resets_the_search_to_the_page_not_above_it() {
    let (mut r, _) = renderer(true);
    ctrl(&mut r, Keycode::F);
    assert_eq!(r.coordinate, Coordinate::ExtendedSearch);
    // Everything on the page is searched, users inside their group too, and
    // nothing carries the provider's name.
    assert!(rows(&r).iter().any(|l| l.contains("bob")), "{:?}", rows(&r));
    assert!(
        r.total_list
            .iter()
            .all(|i| !i.data.as_deref().unwrap_or("").contains("Sign in")),
        "no 'Sign in >' crumbs"
    );
    r.last_keypress_time = handlers::sdl_ticks();
    ctrl(&mut r, Keycode::F);
    assert_eq!(r.current_id.depth(), 2, "the double tap stops at the page");
}

#[test]
fn ctrl_w_leaves_the_provider_name_out() {
    let (mut r, _) = renderer(true);
    r.speak_focus_position();
    let said = r.pending_announcement.clone().unwrap_or_default();
    assert!(said.contains("Password"), "{said}");
    assert!(!said.contains("Sign in"), "{said}");
    assert!(!said.contains("arrow left"), "{said}");
}

// ---- a refresh while the manual is open -------------------------------------

#[test]
fn a_refresh_waits_while_the_manual_is_open_and_runs_after_it() {
    let (mut r, refresh) = renderer(true);
    key(&mut r, Keycode::Tab);
    assert_eq!(r.coordinate, Coordinate::Manual);
    key(&mut r, Keycode::Down);
    key(&mut r, Keycode::Down);
    let manual = rows(&r);

    // The clock ticks: the manual stays as it is, on the same line.
    refresh.store(true, Ordering::SeqCst);
    view::drain_needs_refresh(&mut r);
    assert_eq!(rows(&r), manual);
    assert_eq!(r.list_index, 2);
    assert_eq!(
        r.current_id,
        id(&[0, PASSWORD_ROW]),
        "the page's place is kept"
    );
    assert!(refresh.load(Ordering::SeqCst), "the signal waits");

    // Searching the manual, or reading it as one page, waits too.
    key(&mut r, Keycode::S);
    assert_eq!(r.coordinate, Coordinate::Scroll);
    view::drain_needs_refresh(&mut r);
    assert!(refresh.load(Ordering::SeqCst));
    key(&mut r, Keycode::Escape);
    assert_eq!(r.coordinate, Coordinate::Manual);

    // Escape is back on the password row, and the refresh runs there.
    key(&mut r, Keycode::Escape);
    assert_eq!(r.coordinate, Coordinate::General);
    view::drain_needs_refresh(&mut r);
    assert!(!refresh.load(Ordering::SeqCst), "the refresh ran");
    assert_eq!(r.current_id, id(&[0, PASSWORD_ROW]));
    assert!(
        r.current_list_item().unwrap().label.contains("Password:"),
        "{:?}",
        r.current_list_item()
    );
}

#[test]
fn f_after_the_manual_searches_the_page() {
    let (mut r, refresh) = renderer(true);
    key(&mut r, Keycode::Tab);
    refresh.store(true, Ordering::SeqCst);
    view::drain_needs_refresh(&mut r);
    key(&mut r, Keycode::Escape);
    view::drain_needs_refresh(&mut r);
    key(&mut r, Keycode::F);
    assert_eq!(r.coordinate, Coordinate::SimpleSearch);
    assert!(
        rows(&r).iter().any(|l| l.contains("Shut down")),
        "{:?}",
        rows(&r)
    );
}

// ---- an embedder's own manual ----------------------------------------------

#[test]
fn the_manual_reads_the_lines_the_embedder_names() {
    let (mut r, _) = renderer(true);
    r.manual = ManualText {
        prefix: crate::list::MANUAL_PREFIX,
        lines: 3,
    };
    key(&mut r, Keycode::Tab);
    assert_eq!(r.total_list.len(), 3);
    assert_eq!(
        r.total_list[0].label,
        format!("- {}", crate::shortcuts::manual_line(1))
    );
}

#[test]
fn a_manual_line_with_no_text_reads_empty_not_its_key() {
    let manual = ManualText {
        prefix: "no-such-manual",
        lines: 1,
    };
    assert_eq!(manual.line(1), "");
}
