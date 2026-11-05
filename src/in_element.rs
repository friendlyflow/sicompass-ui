//! In element mode: moving between the images, links and buttons in one row.
//!
//! Entered from General with `i` (first target) or `a` (last), like insert mode.
//! Left and Right go target by target, skipping plain text, and Up and Down go to
//! the line above or below. Enter activates, Escape leaves. Outside this mode the
//! arrow keys do what they always did, so a row with several targets costs nothing
//! until it is asked for.
//!
//! Enter and `d` on an image open the big view, also here, which is the dashboard
//! showing that one image instead of asking the provider what to show.

use crate::app_state::{AppRenderer, Coordinate};
use crate::{handlers, list, provider};
use sicompass_sdk::ffon::{FfonElement, IdArray, get_ffon_at_id};
use sicompass_sdk::localize::{self, Args};
use sicompass_sdk::tags::{self, Target, TargetCounts, TargetKind};

fn focused_targets(r: &AppRenderer) -> Vec<Target> {
    let Some(raw) = handlers::focused_element_raw(r) else {
        return Vec::new();
    };
    if r.sheets.markup_here() {
        sicompass_sdk::markup::targets(&raw)
    } else {
        tags::targets(&raw)
    }
}

/// Say which link was followed or which button was pressed, so Enter is never silent.
fn announce_activation(r: &mut AppRenderer, target: &Target) {
    let key = match target.kind {
        TargetKind::Link => "target-followed",
        TargetKind::Button => "target-pressed",
        _ => return,
    };
    let mut args = Args::new();
    args.set("name", target.name.clone());
    let fallback = match target.kind {
        TargetKind::Link => format!("followed link, {}", target.name),
        _ => format!("pressed button, {}", target.name),
    };
    let phrase = localized(key, &args, fallback);
    r.announce_provider_line(phrase);
}

fn focused_counts(r: &AppRenderer) -> TargetCounts {
    handlers::focused_element_raw(r)
        .map(|raw| {
            if r.sheets.markup_here() {
                sicompass_sdk::markup::target_counts(&raw)
            } else {
                tags::target_counts(&raw)
            }
        })
        .unwrap_or_default()
}

/// `i` and `a` start in element mode on this row.
pub(crate) fn can_enter(r: &AppRenderer) -> bool {
    r.current_id.depth() > 1 && focused_counts(r).total() > 0
}

/// The row's one target is an image, so Enter and `d` can open it from General.
pub(crate) fn focused_is_only_an_image(r: &AppRenderer) -> bool {
    let counts = focused_counts(r);
    r.current_id.depth() > 1 && counts.images + counts.videos == 1 && counts.total() == 1
}

fn localized(key: &str, args: &Args, fallback: String) -> String {
    crate::shortcuts::register_translations();
    let text = localize::t_args(key, args);
    if text == key { fallback } else { text }
}

fn count_phrase(key: &str, count: usize, singular: &str) -> Option<String> {
    if count == 0 {
        return None;
    }
    let mut args = Args::new();
    args.set("count", count as i64);
    let fallback = if count == 1 {
        format!("1 {singular}")
    } else {
        format!("{count} {singular}s")
    };
    Some(localized(key, &args, fallback))
}

/// What landing on a row says about what it holds: "2 links, 3 images, 1 button".
/// Empty for a row with none.
pub(crate) fn counts_phrase(counts: TargetCounts) -> String {
    [
        count_phrase("targets-links", counts.links, "link"),
        count_phrase("targets-images", counts.images, "image"),
        count_phrase("targets-buttons", counts.buttons, "button"),
        count_phrase("targets-videos", counts.videos, "video"),
    ]
    .into_iter()
    .flatten()
    .collect::<Vec<_>>()
    .join(", ")
}

/// "link 2 of 3, About". The number counts its own kind, because that is what the
/// row said on landing.
fn target_phrase(targets: &[Target], index: usize) -> String {
    let Some(target) = targets.get(index) else {
        return String::new();
    };
    let (key, word) = match target.kind {
        TargetKind::Link => ("target-link", "link"),
        TargetKind::Image => ("target-image", "image"),
        TargetKind::Button => ("target-button", "button"),
        TargetKind::Video => ("target-video", "video"),
    };
    crate::shortcuts::register_translations();
    let kind = localize::t(key);
    let kind = if kind == key { word.to_owned() } else { kind };
    let same_kind = |t: &&Target| t.kind == target.kind;
    let n = targets[..=index].iter().filter(same_kind).count();
    let total = targets.iter().filter(same_kind).count();
    let mut args = Args::new();
    args.set("kind", kind.clone());
    args.set("n", n as i64);
    args.set("total", total as i64);
    args.set("name", target.name.clone());
    localized(
        "target-position",
        &args,
        format!("{kind} {n} of {total}, {}", target.name),
    )
}

fn announce_target(r: &mut AppRenderer) {
    let phrase = target_phrase(&r.nav_targets, r.nav_index);
    r.announce_provider_line(phrase);
    r.needs_redraw = true;
}

fn enter(r: &mut AppRenderer, last: bool) {
    if !r.coordinate.is_general() {
        return;
    }
    let targets = focused_targets(r);
    if targets.is_empty() {
        return;
    }
    r.nav_index = if last { targets.len() - 1 } else { 0 };
    r.nav_targets = targets;
    r.nav_positions.clear();
    r.nav_return = r.coordinate;
    r.coordinate = Coordinate::InElement;
    let phrase = target_phrase(&r.nav_targets, r.nav_index);
    r.speak_mode_change(Some(phrase));
    r.needs_redraw = true;
}

/// `i`: in element mode on the row's first target.
pub fn handle_first(r: &mut AppRenderer) {
    enter(r, false);
}

/// `a`: in element mode on the row's last target.
pub fn handle_last(r: &mut AppRenderer) {
    enter(r, true);
}

fn leave(r: &mut AppRenderer) {
    r.coordinate = r.nav_return;
    r.nav_targets.clear();
    r.nav_positions.clear();
    r.nav_index = 0;
    r.needs_redraw = true;
}

/// Escape: back to the mode it came from, on the same row.
pub fn handle_escape(r: &mut AppRenderer) {
    leave(r);
    r.speak_mode_change(None);
}

/// Staying put still speaks, so a key at an edge is never silent.
fn move_to(r: &mut AppRenderer, index: usize) {
    r.nav_index = index;
    announce_target(r);
}

pub fn handle_right(r: &mut AppRenderer) {
    let last = r.nav_targets.len().saturating_sub(1);
    move_to(r, (r.nav_index + 1).min(last));
}

pub fn handle_left(r: &mut AppRenderer) {
    move_to(r, r.nav_index.saturating_sub(1));
}

fn move_line(r: &mut AppRenderer, down: bool) {
    let next = crate::layout::target_on_next_line(&r.nav_positions, r.nav_index, down);
    move_to(r, next.unwrap_or(r.nav_index));
}

pub fn handle_up(r: &mut AppRenderer) {
    move_line(r, false);
}

pub fn handle_down(r: &mut AppRenderer) {
    move_line(r, true);
}

/// The list was rebuilt under the mode, by a provider refresh say. Stay on the same
/// target if it still exists, and leave if the row has none any more.
pub(crate) fn after_list_rebuild(r: &mut AppRenderer) {
    if r.coordinate != Coordinate::InElement {
        return;
    }
    let targets = focused_targets(r);
    if targets.is_empty() {
        handle_escape(r);
        return;
    }
    r.nav_index = r.nav_index.min(targets.len() - 1);
    r.nav_targets = targets;
}

/// Where the selected target is in `label`, by character, for the highlight.
pub(crate) fn highlight_positions(r: &AppRenderer, label: &str) -> Vec<u32> {
    let Some(target) = r.nav_targets.get(r.nav_index) else {
        return Vec::new();
    };
    let prefix = crate::view::split_label(label).0.chars().count() as u32;
    target
        .display_span
        .clone()
        .map(|c| c as u32 + prefix)
        .collect()
}

/// The selected target is an image, which gets a frame instead of a text highlight.
pub(crate) fn selected_is_image(r: &AppRenderer) -> bool {
    r.coordinate == Coordinate::InElement
        && r.nav_targets
            .get(r.nav_index)
            .is_some_and(|t| t.kind == TargetKind::Image)
}

/// Enter: activate the selected target.
pub fn handle_enter(r: &mut AppRenderer) {
    let Some(target) = r.nav_targets.get(r.nav_index).cloned() else {
        return;
    };
    match target.kind {
        TargetKind::Link => {
            let (id, return_to) = (r.current_id.clone(), r.nav_return);
            // Which of the row's links this is, for the parent line of the page.
            let which = r.nav_targets[..=r.nav_index]
                .iter()
                .filter(|t| t.kind == TargetKind::Link)
                .count()
                .saturating_sub(1);
            r.coordinate = return_to;
            if follow_link(r, &target.value) {
                r.followed_link = Some((id.clone(), which, target.value.clone()));
                announce_activation(r, &target);
                r.nav_targets.clear();
                r.nav_positions.clear();
            } else {
                // Nothing there: stay on it and say so again.
                r.current_id = id;
                r.coordinate = Coordinate::InElement;
                announce_target(r);
            }
        }
        TargetKind::Button => press_button(r, &target),
        TargetKind::Image | TargetKind::Video => open_image_view(r),
    }
}

/// Run a button, staying in the mode if the row is still there afterwards.
fn press_button(r: &mut AppRenderer, target: &Target) {
    announce_activation(r, target);
    let row = r.current_id.clone();
    let return_to = r.nav_return;
    r.coordinate = return_to;
    if r.nav_targets.len() == 1 {
        // The row's only button goes through everything Enter does for a button:
        // a session list's commands and a history row's refill.
        handlers::handle_enter_general(r);
    } else {
        provider::notify_button_function(r, &target.value);
        let error = std::mem::take(&mut r.error_message);
        list::create_list_current_layer(r);
        if r.error_message.is_empty() {
            r.error_message = error;
        }
    }
    r.needs_redraw = true;
    if r.coordinate == return_to && r.current_id == row && !focused_targets(r).is_empty() {
        r.coordinate = Coordinate::InElement;
        after_list_rebuild(r);
    }
}

/// Follow `url` from the focused row, landing inside the page.
///
/// One row can hold links to several pages but only one list of children, so the
/// row holds the page last followed, and following another replaces it. A `Str` row
/// becomes an `Obj` to hold it. This is only in the app's tree, as link pages always
/// are: the provider never sees it.
pub(crate) fn follow_link(r: &mut AppRenderer, url: &str) -> bool {
    let row = r.current_id.clone();
    let path_before = (row.depth() >= 2).then(|| provider::current_path(r).to_owned());
    if let Some(fragment) = url.strip_prefix('#') {
        return handlers::jump_to_fragment(r, &row, fragment);
    }
    let (children, is_document) = handlers::resolve_link(url);
    if children.is_empty() {
        return false;
    }
    let index = row.last().unwrap_or(0);
    let Some(siblings) = provider::get_ffon_at_id_mut(&mut r.ffon, &row) else {
        return false;
    };
    let Some(element) = siblings.get_mut(index) else {
        return false;
    };
    match element {
        FfonElement::Obj(obj) => obj.children = children,
        FfonElement::Str(text) => {
            let mut obj = FfonElement::new_obj(std::mem::take(text));
            if let Some(o) = obj.as_obj_mut() {
                o.children = children;
            }
            *element = obj;
        }
    }
    if is_document {
        handlers::mark_document(r, &row);
    }
    let mut inside: IdArray = row.clone();
    inside.push(0);
    r.current_id = inside;
    handlers::finish_right(r, row, path_before);
    true
}

/// Enter on a row in General that shows "Enter Follow link".
pub fn handle_enter_link(r: &mut AppRenderer) {
    let raw = handlers::focused_element_raw(r).unwrap_or_default();
    let raw = if r.sheets.markup_here() {
        sicompass_sdk::markup::plain(&raw)
    } else {
        raw
    };
    let is_plain_link = !tags::has_button(&raw)
        && !tags::has_checkbox(&raw)
        && !tags::has_checkbox_checked(&raw)
        && !tags::has_input(&raw);
    if !is_plain_link {
        handlers::handle_enter_general(r);
        return;
    }
    let first_link = focused_targets(r)
        .into_iter()
        .find(|t| t.kind == TargetKind::Link);
    let row_before = r.current_id.clone();
    let index = r.current_id.last().unwrap_or(0);
    let is_obj = get_ffon_at_id(&r.ffon, &r.current_id)
        .and_then(|slice| slice.get(index))
        .is_some_and(|e| matches!(e, FfonElement::Obj(_)));
    if is_obj {
        // A link row with a page under it: Enter goes in, and says which link it was.
        let row = r.current_id.clone();
        handlers::handle_right(r);
        if r.current_id.depth() > row.depth()
            && let Some(first) = first_link
        {
            r.followed_link = Some((row, 0, first.value.clone()));
            announce_activation(r, &first);
        }
    } else if let Some(url) = tags::extract_link(&raw)
        && follow_link(r, &url)
        && let Some(first) = first_link
    {
        r.followed_link = Some((row_before, 0, first.value.clone()));
        announce_activation(r, &first);
    }
}

/// Enter on a row holding a button, in General: it presses the first, and says which.
pub fn handle_enter_button(r: &mut AppRenderer) {
    if let Some(first) = focused_targets(r)
        .into_iter()
        .find(|t| t.kind == TargetKind::Button)
    {
        announce_activation(r, &first);
    }
    handlers::handle_enter_general(r);
}

/// Enter or `d` on a row whose one target is an image.
pub fn handle_view_only_image(r: &mut AppRenderer) {
    r.nav_targets = focused_targets(r);
    r.nav_index = 0;
    open_image_view(r);
}

fn show_image(r: &mut AppRenderer) {
    if let Some(target) = r.nav_targets.get(r.nav_index).cloned() {
        if target.kind == TargetKind::Video {
            // The frame loop opens it and starts it, and the picture is its.
            r.dashboard_image_path.clear();
            r.video_open = Some(target.value.clone());
            r.video_request = Some(crate::video::Request::Open(target.value));
        } else {
            r.dashboard_image_path = target.value;
            if r.video_open.take().is_some() {
                r.video_request = Some(crate::video::Request::Stop);
            }
        }
    }
    r.needs_redraw = true;
}

/// The big view of the selected image or video.
fn open_image_view(r: &mut AppRenderer) {
    open_image_view_with(r, crate::video::available());
}

fn open_image_view_with(r: &mut AppRenderer, video_available: bool) {
    let is_video = r
        .nav_targets
        .get(r.nav_index)
        .is_some_and(|t| t.kind == TargetKind::Video);
    if is_video && !video_available {
        // Said, and nothing opens: a black window would say less.
        r.error_message = localize::t("video-unavailable");
        r.needs_redraw = true;
        return;
    }
    show_image(r);
    r.element_image_view = true;
    r.previous_coordinate = r.coordinate;
    r.coordinate = Coordinate::Dashboard;
    let phrase = target_phrase(&r.nav_targets, r.nav_index);
    r.speak_mode_change(Some(phrase));
}

/// Enter in the big view: pause the video, start it again, or play it again from the
/// start if it has ended.
pub fn handle_toggle_video(r: &mut AppRenderer) {
    if r.video_open.is_some() {
        r.video_request = Some(crate::video::Request::TogglePause);
        r.needs_redraw = true;
    }
}

/// A video is open in the big view.
pub(crate) fn video_is_open(r: &AppRenderer) -> bool {
    in_image_view(r) && r.video_open.is_some()
}

/// True while the dashboard is showing an image from a row.
pub(crate) fn in_image_view(r: &AppRenderer) -> bool {
    r.coordinate == Coordinate::Dashboard && r.element_image_view
}

/// The image `step` images on from the current one in the row, skipping other kinds.
fn image_step(targets: &[Target], from: usize, forward: bool) -> Option<usize> {
    let is_image = |i: &usize| matches!(targets[*i].kind, TargetKind::Image | TargetKind::Video);
    if forward {
        (from + 1..targets.len()).find(is_image)
    } else {
        (0..from).rev().find(is_image)
    }
}

fn step_image(r: &mut AppRenderer, forward: bool) {
    if let Some(next) = image_step(&r.nav_targets, r.nav_index, forward) {
        r.nav_index = next;
        show_image(r);
    }
    announce_target(r);
}

pub fn handle_image_next(r: &mut AppRenderer) {
    step_image(r, true);
}

pub fn handle_image_previous(r: &mut AppRenderer) {
    step_image(r, false);
}

/// Ctrl+C and Ctrl+Shift+C: the selected target's address, path or function.
pub fn handle_copy(r: &mut AppRenderer) {
    if let Some(value) = r.nav_targets.get(r.nav_index).map(|t| t.value.clone()) {
        handlers::sdl_set_clipboard(&value);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use sdl3::keyboard::{Keycode, Mod};

    const THREE_LINKS: &str =
        "See One <link>u1</link> and Two <link>u2</link> or Three <link>u3</link>";

    /// A one-level list inside a provider: `rows` are the strings, cursor on `at`.
    fn renderer(rows: &[&str], at: usize) -> AppRenderer {
        let mut root = FfonElement::new_obj("root");
        for row in rows {
            root.as_obj_mut().unwrap().push(FfonElement::new_str(*row));
        }
        let mut r = AppRenderer::new();
        r.ffon = vec![root];
        let mut id = IdArray::new();
        id.push(0);
        id.push(at);
        r.current_id = id;
        list::create_list_current_layer(&mut r);
        r
    }

    fn press(r: &mut AppRenderer, key: Keycode) {
        crate::shortcuts::dispatch_key(r, Some(key), Mod::NOMOD);
    }

    fn announced(r: &AppRenderer) -> String {
        r.pending_announcement
            .as_deref()
            .unwrap_or("")
            .trim_end_matches('\u{200B}')
            .to_owned()
    }

    /// Enter says which link it follows and which button it presses, with or without
    /// element mode, and the first one when the row has several.
    #[test]
    fn enter_says_which_link_or_button_it_used() {
        // Rows 0 and 1 are what the fragments point to, and row 2 holds the links.
        let rows = [
            "<id>a</id>Alpha",
            "<id>b</id>Beta",
            "Home <link>#a</link> and About <link>#b</link>",
        ];
        let mut r = renderer(&rows, 2);
        press(&mut r, Keycode::Return);
        assert!(
            announced(&r).starts_with("followed link, Home"),
            "{}",
            announced(&r)
        );

        let mut r = renderer(&rows, 2);
        press(&mut r, Keycode::A);
        press(&mut r, Keycode::Return);
        assert!(
            announced(&r).starts_with("followed link, and About"),
            "{}",
            announced(&r)
        );

        let row = "<button>one</button>First <button>two</button>Second";
        let mut r = renderer(&[row], 0);
        press(&mut r, Keycode::Return);
        assert!(
            announced(&r).starts_with("pressed button, First"),
            "{}",
            announced(&r)
        );

        let mut r = renderer(&[row], 0);
        press(&mut r, Keycode::A);
        press(&mut r, Keycode::Return);
        assert!(
            announced(&r).starts_with("pressed button, Second"),
            "{}",
            announced(&r)
        );
    }

    /// `i` and `a` and the highlight have to read the row without its markup.
    #[test]
    fn targets_inside_markup_are_found_at_what_the_row_shows() {
        struct Markup;
        impl sicompass_sdk::provider::Provider for Markup {
            fn name(&self) -> &str {
                "markup"
            }
            fn fetch(&mut self) -> Vec<FfonElement> {
                Vec::new()
            }
            fn markup(&self) -> bool {
                true
            }
        }
        let row = r#"<div class="x">One <link>u1</link> <b>Two</b> <link>u2</link></div>"#;
        let mut r = renderer(&[row], 0);
        r.providers.push(Box::new(Markup));
        list::create_list_current_layer(&mut r);
        assert_eq!(r.total_list[0].label, "-l One u1 Two u2");
        press(&mut r, Keycode::A);
        assert_eq!(r.nav_targets.len(), 2);
        let label = r.total_list[0].label.clone();
        let chars: Vec<char> = label.chars().collect();
        let shown: String = highlight_positions(&r, &label)
            .iter()
            .map(|&p| chars[p as usize])
            .collect();
        assert_eq!(shown, "Two u2");
    }

    #[test]
    fn i_starts_on_the_first_target_and_says_so() {
        let mut r = renderer(&[THREE_LINKS], 0);
        press(&mut r, Keycode::I);
        assert_eq!(r.coordinate, Coordinate::InElement);
        assert_eq!(r.nav_index, 0);
        let said = announced(&r);
        assert!(said.contains("1"), "{said}");
        assert!(said.contains("See One"), "{said}");
    }

    #[test]
    fn a_starts_on_the_last_target() {
        let mut r = renderer(&[THREE_LINKS], 0);
        press(&mut r, Keycode::A);
        assert_eq!(r.coordinate, Coordinate::InElement);
        assert_eq!(r.nav_index, 2);
    }

    #[test]
    fn a_row_with_no_targets_does_not_start_it() {
        let mut r = renderer(&["plain text"], 0);
        press(&mut r, Keycode::I);
        assert_eq!(r.coordinate, Coordinate::General);
    }

    /// `i` keeps its old meaning where it had one.
    #[test]
    fn a_row_with_an_input_still_edits() {
        let mut r = renderer(&["<input>x</input> <link>u</link>"], 0);
        press(&mut r, Keycode::I);
        assert_eq!(r.coordinate, Coordinate::Insert);
    }

    #[test]
    fn right_and_left_move_by_target_and_stop_at_the_ends() {
        let mut r = renderer(&[THREE_LINKS], 0);
        press(&mut r, Keycode::I);
        press(&mut r, Keycode::Left);
        assert_eq!(r.nav_index, 0, "stops at the first");
        press(&mut r, Keycode::Right);
        assert_eq!(r.nav_index, 1);
        assert!(announced(&r).contains("and Two"));
        press(&mut r, Keycode::Right);
        press(&mut r, Keycode::Right);
        assert_eq!(r.nav_index, 2, "stops at the last");
        press(&mut r, Keycode::Left);
        assert_eq!(r.nav_index, 1);
    }

    /// A key at an edge is never silent: the current target is said again, and a
    /// changed marker makes the screen reader hear a repeat of the same words.
    #[test]
    fn an_edge_repeats_the_current_target() {
        let mut r = renderer(&[THREE_LINKS], 0);
        press(&mut r, Keycode::I);
        press(&mut r, Keycode::Left);
        let first = r.pending_announcement.clone();
        press(&mut r, Keycode::Left);
        assert!(announced(&r).contains("See One"));
        assert_ne!(r.pending_announcement, first);
    }

    #[test]
    fn up_and_down_go_by_line_using_where_the_targets_are() {
        use crate::layout::TargetPos;
        let mut r = renderer(&[THREE_LINKS], 0);
        press(&mut r, Keycode::I);
        r.nav_positions = vec![
            TargetPos { x: 0.0, y: 0.0 },
            TargetPos { x: 50.0, y: 0.0 },
            TargetPos { x: 40.0, y: 20.0 },
        ];
        r.nav_index = 1;
        press(&mut r, Keycode::Down);
        assert_eq!(r.nav_index, 2);
        press(&mut r, Keycode::Down);
        assert_eq!(r.nav_index, 2, "no line below");
        press(&mut r, Keycode::Up);
        assert_eq!(r.nav_index, 1, "the nearest in x on the line above");
    }

    #[test]
    fn escape_goes_back_to_the_same_row() {
        let mut r = renderer(&["x", THREE_LINKS, "y"], 1);
        press(&mut r, Keycode::I);
        press(&mut r, Keycode::Right);
        press(&mut r, Keycode::Escape);
        assert_eq!(r.coordinate, Coordinate::General);
        assert_eq!(r.current_id.last(), Some(1));
        assert!(r.nav_targets.is_empty());
    }

    /// Outside the mode the arrows are what they always were.
    #[test]
    fn outside_the_mode_down_moves_to_the_next_row() {
        let mut r = renderer(&[THREE_LINKS, "y"], 0);
        press(&mut r, Keycode::Right);
        assert_eq!(r.current_id.depth(), 2, "Right on a Str stays");
        press(&mut r, Keycode::Down);
        assert_eq!(r.current_id.last(), Some(1));
    }

    #[test]
    fn a_refresh_clamps_to_the_targets_that_are_left() {
        let mut r = renderer(&[THREE_LINKS], 0);
        press(&mut r, Keycode::A);
        assert_eq!(r.nav_index, 2);
        *r.ffon[0].as_obj_mut().unwrap().children.get_mut(0).unwrap() =
            FfonElement::new_str("Only <link>u</link>");
        list::create_list_current_layer(&mut r);
        assert_eq!(r.coordinate, Coordinate::InElement);
        assert_eq!(r.nav_index, 0);
        assert_eq!(r.nav_targets.len(), 1);
    }

    #[test]
    fn a_refresh_that_takes_every_target_leaves_the_mode() {
        let mut r = renderer(&[THREE_LINKS], 0);
        press(&mut r, Keycode::I);
        *r.ffon[0].as_obj_mut().unwrap().children.get_mut(0).unwrap() =
            FfonElement::new_str("gone");
        list::create_list_current_layer(&mut r);
        assert_eq!(r.coordinate, Coordinate::General);
    }

    #[test]
    fn the_highlight_is_the_selected_target_after_the_prefix() {
        let mut r = renderer(&["See <link>u1</link> or <link>u2</link>"], 0);
        press(&mut r, Keycode::A);
        // "- See u1 or u2": the second target is "or u2", after the prefix "- ".
        let label = r.total_list[0].label.clone();
        let chars: Vec<char> = label.chars().collect();
        let shown: String = highlight_positions(&r, &label)
            .iter()
            .map(|&p| chars[p as usize])
            .collect();
        assert_eq!(shown, "or u2");
    }

    #[test]
    fn counts_name_each_kind_and_skip_the_ones_it_has_none_of() {
        let phrase = counts_phrase(TargetCounts {
            links: 2,
            images: 3,
            buttons: 1,
            videos: 0,
        });
        assert!(
            phrase.contains('2') && phrase.contains('3') && phrase.contains('1'),
            "{phrase}"
        );
        assert_eq!(phrase.matches(", ").count(), 2, "{phrase}");
        let only = counts_phrase(TargetCounts {
            links: 1,
            ..Default::default()
        });
        assert_eq!(only.matches(", ").count(), 0, "{only}");
        assert_eq!(counts_phrase(TargetCounts::default()), "");
    }

    #[test]
    fn each_kind_counts_on_its_own() {
        let targets = tags::targets("a <link>u</link><button>f</button>Go b <link>v</link>");
        let phrase = target_phrase(&targets, 2);
        assert!(phrase.contains('2'), "second link: {phrase}");
        let button = target_phrase(&targets, 1);
        assert!(button.contains('1'), "first button: {button}");
    }

    /// Following a Str row turns it into a row that holds the page.
    #[test]
    fn enter_on_a_link_follows_the_one_selected() {
        let dir = std::env::temp_dir().join(format!("nav-link-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let (one, two) = (dir.join("one.json"), dir.join("two.json"));
        std::fs::write(&one, r#"["page one"]"#).unwrap();
        std::fs::write(&two, r#"["page two"]"#).unwrap();
        let row = format!(
            "A <link>{}</link> B <link>{}</link>",
            one.display(),
            two.display()
        );
        let mut r = renderer(&[row.as_str()], 0);
        press(&mut r, Keycode::I);
        press(&mut r, Keycode::Right);
        press(&mut r, Keycode::Return);
        assert_eq!(r.coordinate, Coordinate::General);
        assert_eq!(r.current_id.depth(), 3, "inside the page");
        assert!(
            r.total_list[0].label.contains("page two"),
            "{:?}",
            r.total_list[0].label
        );
        std::fs::remove_dir_all(&dir).ok();
    }

    /// The hint said so all along and nothing happened.
    #[test]
    fn enter_in_general_follows_a_rows_only_link() {
        let dir = std::env::temp_dir().join(format!("nav-only-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let page = dir.join("p.json");
        std::fs::write(&page, r#"["the page"]"#).unwrap();
        let row = format!("Go <link>{}</link>", page.display());
        let mut r = renderer(&[row.as_str()], 0);
        press(&mut r, Keycode::Return);
        assert_eq!(r.current_id.depth(), 3);
        assert!(r.total_list[0].label.contains("the page"));
        std::fs::remove_dir_all(&dir).ok();
    }

    /// A real `<a href>` is followed just like the older tag.
    #[test]
    fn enter_follows_a_real_anchor_in_a_markup_row() {
        struct Markup;
        impl sicompass_sdk::provider::Provider for Markup {
            fn name(&self) -> &str {
                "markup"
            }
            fn fetch(&mut self) -> Vec<FfonElement> {
                Vec::new()
            }
            fn markup(&self) -> bool {
                true
            }
        }
        let dir = std::env::temp_dir().join(format!("nav-anchor-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let page = dir.join("p.json");
        std::fs::write(&page, r#"["the page"]"#).unwrap();
        let row = format!(r#"Go <a href="{}">there</a>"#, page.display());
        let mut r = renderer(&[row.as_str()], 0);
        r.providers.push(Box::new(Markup));
        list::create_list_current_layer(&mut r);
        press(&mut r, Keycode::Return);
        assert_eq!(r.current_id.depth(), 3, "inside the page");
        assert!(r.total_list[0].label.contains("the page"));
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn a_dead_link_stays_where_it_is() {
        let row = "Go <link>/no/such/file.json</link> or <link>/none.json</link>";
        let mut r = renderer(&[row], 0);
        press(&mut r, Keycode::I);
        press(&mut r, Keycode::Return);
        assert_eq!(r.coordinate, Coordinate::InElement);
        assert_eq!(r.current_id.depth(), 2);
    }

    #[test]
    fn enter_and_d_open_the_selected_image_big() {
        for key in [Keycode::Return, Keycode::D] {
            let mut r = renderer(&["<image>a.jpg</image><image>b.jpg</image>"], 0);
            press(&mut r, Keycode::A);
            press(&mut r, key);
            assert_eq!(r.coordinate, Coordinate::Dashboard, "{key:?}");
            assert!(in_image_view(&r));
            assert_eq!(r.dashboard_image_path, "b.jpg");
        }
    }

    fn open_video(row: &str, at_last: bool, available: bool) -> AppRenderer {
        struct Markup;
        impl sicompass_sdk::provider::Provider for Markup {
            fn name(&self) -> &str {
                "markup"
            }
            fn fetch(&mut self) -> Vec<FfonElement> {
                Vec::new()
            }
            fn markup(&self) -> bool {
                true
            }
        }
        let mut r = renderer(&[row], 0);
        r.providers.push(Box::new(Markup));
        list::create_list_current_layer(&mut r);
        press(&mut r, if at_last { Keycode::A } else { Keycode::I });
        open_image_view_with(&mut r, available);
        r
    }

    const MIXED: &str =
        r#"<img src="a.jpg"><video src="/clips/v.mp4">Clip</video><img src="b.jpg">"#;

    #[test]
    fn a_video_opens_in_the_big_view_and_asks_to_be_played() {
        let r = open_video(r#"<video src="/clips/v.mp4">Clip</video>"#, false, true);
        assert_eq!(r.coordinate, Coordinate::Dashboard);
        assert!(in_image_view(&r));
        assert_eq!(r.video_open.as_deref(), Some("/clips/v.mp4"));
        assert_eq!(
            r.video_request,
            Some(crate::video::Request::Open("/clips/v.mp4".to_owned()))
        );
        assert!(r.dashboard_image_path.is_empty());
        assert!(announced(&r).contains("Clip"), "{}", announced(&r));
    }

    /// With no decoder the window must say so, and not open on nothing.
    #[test]
    fn a_video_with_no_backend_says_so_and_stays_put() {
        let r = open_video(r#"<video src="/clips/v.mp4"></video>"#, false, false);
        assert_eq!(r.coordinate, Coordinate::InElement);
        assert!(r.video_request.is_none() && r.video_open.is_none());
        assert!(!r.error_message.is_empty());
    }

    #[test]
    fn enter_is_the_key_that_pauses_and_space_is_not() {
        let mut r = open_video(r#"<video src="/clips/v.mp4"></video>"#, false, true);
        r.video_request = None;
        press(&mut r, Keycode::Space);
        assert!(r.video_request.is_none(), "Space does nothing to a video");
        press(&mut r, Keycode::Return);
        assert_eq!(r.video_request, Some(crate::video::Request::TogglePause));
        assert_eq!(
            r.coordinate,
            Coordinate::Dashboard,
            "it stays in the big view"
        );
    }

    /// Enter on a picture in the big view is not a pause, and leaves it open.
    #[test]
    fn enter_on_a_picture_in_the_big_view_asks_for_nothing() {
        let mut r = renderer(&["<image>a.jpg</image>"], 0);
        press(&mut r, Keycode::D);
        assert!(in_image_view(&r));
        press(&mut r, Keycode::Return);
        assert!(r.video_request.is_none());
        assert!(in_image_view(&r));
    }

    #[test]
    fn enter_pauses_only_while_a_video_is_open() {
        let mut r = open_video(r#"<video src="/clips/v.mp4"></video>"#, false, true);
        r.video_request = None;
        handle_toggle_video(&mut r);
        assert_eq!(r.video_request, Some(crate::video::Request::TogglePause));
        let mut r = renderer(&["<image>a.jpg</image>"], 0);
        press(&mut r, Keycode::D);
        handle_toggle_video(&mut r);
        assert!(r.video_request.is_none(), "an image has nothing to pause");
    }

    #[test]
    fn leaving_the_big_view_stops_the_video() {
        let mut r = open_video(r#"<video src="/clips/v.mp4"></video>"#, false, true);
        r.video_request = None;
        crate::handlers::handle_dashboard_leave(&mut r);
        assert_eq!(r.video_request, Some(crate::video::Request::Stop));
        assert!(r.video_open.is_none());
    }

    /// Left and Right in the big view go over pictures and videos alike, and each step
    /// starts or stops the player as it should.
    #[test]
    fn stepping_over_a_mixed_row_opens_and_stops_the_video() {
        let mut r = open_video(MIXED, false, true);
        assert_eq!(r.video_open, None, "it opened on the first picture");
        r.nav_index = 1;
        show_image(&mut r);
        assert_eq!(
            r.video_request,
            Some(crate::video::Request::Open("/clips/v.mp4".to_owned()))
        );
        r.nav_index = 2;
        r.video_request = None;
        show_image(&mut r);
        assert_eq!(r.video_request, Some(crate::video::Request::Stop));
        assert_eq!(r.dashboard_image_path, "b.jpg");
        assert!(r.video_open.is_none());
        assert_eq!(image_step(&r.nav_targets, 0, true), Some(1));
        assert_eq!(image_step(&r.nav_targets, 2, false), Some(1));
    }

    #[test]
    fn a_rows_only_video_opens_from_general_with_enter_or_d() {
        let mut r = open_video(r#"<video src="/clips/v.mp4"></video>"#, false, true);
        press(&mut r, Keycode::Escape);
        assert_eq!(r.coordinate, Coordinate::InElement);
        assert!(focused_is_only_an_image(&r));
    }

    #[test]
    fn enter_and_d_open_a_rows_only_image_from_general() {
        for key in [Keycode::Return, Keycode::D] {
            let mut r = renderer(&["A cat <image>c.jpg</image>"], 0);
            press(&mut r, key);
            assert!(in_image_view(&r), "{key:?}");
            assert_eq!(r.dashboard_image_path, "c.jpg");
            assert!(announced(&r).contains("A cat"));
        }
    }

    #[test]
    fn the_big_view_steps_through_images_only_and_returns_to_its_target() {
        let row = "<image>a.jpg</image> <link>u</link> <image>b.jpg</image>";
        let mut r = renderer(&[row], 0);
        press(&mut r, Keycode::I);
        press(&mut r, Keycode::Return);
        press(&mut r, Keycode::Right);
        assert_eq!(r.dashboard_image_path, "b.jpg", "skips the link");
        assert_eq!(r.nav_index, 2);
        press(&mut r, Keycode::Right);
        assert_eq!(r.dashboard_image_path, "b.jpg", "stops at the last");
        press(&mut r, Keycode::Escape);
        assert_eq!(r.coordinate, Coordinate::InElement);
        assert!(!r.element_image_view);
        assert_eq!(r.nav_index, 2);
        press(&mut r, Keycode::Escape);
        assert_eq!(r.coordinate, Coordinate::General);
    }
}
