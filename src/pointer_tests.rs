//! The mouse, the touchpad and the touch screen (`crate::pointer`), driven through
//! the entry points the event loop calls, against a hit map filled the way a
//! frame fills it.

use std::sync::{Arc, Mutex};

use sicompass_sdk::ffon::{FfonElement, IdArray};
use sicompass_sdk::provider::Provider;

use crate::app_state::{AppRenderer, Coordinate};
use crate::pointer::{self, Button, CaretLine, CellGrid, Hit, HitMap, Rect};
use crate::{list, registry};

type Log = Arc<Mutex<Vec<String>>>;

/// A level with one of each kind of row a click does something different with.
struct Desk {
    path: String,
    log: Log,
    interactive: bool,
}

const CHECKBOX: usize = 0;
const BUTTON: usize = 1;
const FOLDER: usize = 2;
const INPUT: usize = 3;
const TWO_BUTTONS: usize = 4;
const PLAIN: usize = 5;
const OTHER_INPUT: usize = 6;

fn level() -> Vec<FfonElement> {
    let mut folder = FfonElement::new_obj("Folder");
    let o = folder.as_obj_mut().unwrap();
    o.push(FfonElement::Str("inside one".to_owned()));
    o.push(FfonElement::Str("inside two".to_owned()));
    vec![
        FfonElement::Str(sicompass_sdk::tags::format_checkbox("Wifi")),
        FfonElement::Str("<button>do_it</button>Do it".to_owned()),
        folder,
        FfonElement::Str("<input>hello world</input>".to_owned()),
        FfonElement::Str("<button>first</button>One <button>second</button>Two".to_owned()),
        FfonElement::Str("plain".to_owned()),
        FfonElement::Str("<input>other</input>".to_owned()),
    ]
}

impl Provider for Desk {
    fn name(&self) -> &str {
        "desk"
    }
    fn fetch(&mut self) -> Vec<FfonElement> {
        match self.path.as_str() {
            "/Folder" => level()[FOLDER].as_obj().unwrap().children.clone(),
            _ => level(),
        }
    }
    fn push_path(&mut self, seg: &str) {
        self.path = format!("/{}", sicompass_sdk::tags::strip_display(seg));
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
        self.log.lock().unwrap().push(format!("button {function_name}"));
    }
    fn on_checkbox_change(&mut self, label: &str, checked: bool) {
        self.log
            .lock()
            .unwrap()
            .push(format!("checkbox {} {checked}", sicompass_sdk::tags::strip_display(label)));
    }
    fn commit_edit(&mut self, old: &str, new: &str) -> bool {
        self.log.lock().unwrap().push(format!("commit {old} -> {new}"));
        true
    }
    fn dashboard_kind(&self) -> sicompass_sdk::DashboardKind {
        if self.interactive {
            sicompass_sdk::DashboardKind::Interactive
        } else {
            sicompass_sdk::DashboardKind::None
        }
    }
    fn dashboard_mouse(&mut self, ev: sicompass_sdk::DashboardMouse) -> bool {
        self.log.lock().unwrap().push(format!(
            "mouse {:?} {:?} {},{}",
            ev.kind, ev.button, ev.col, ev.row
        ));
        true
    }
    fn leave_dashboard(&mut self) {
        self.log.lock().unwrap().push("leave dashboard".to_owned());
    }
}

fn id(parts: &[usize]) -> IdArray {
    let mut a = IdArray::new();
    for &p in parts {
        a.push(p);
    }
    a
}

const ROW_H: f32 = 20.0;
/// Where the list starts, under the header and the parent line.
const LIST_TOP: f32 = 2.0 * ROW_H;

fn row_y(n: usize) -> f32 {
    LIST_TOP + n as f32 * ROW_H + ROW_H / 2.0
}

/// The hit map a frame of this level would leave: the parent line and one band
/// per row, the two-button row's buttons on top of it.
fn frame(r: &mut AppRenderer) {
    let mut map = HitMap {
        line_height: ROW_H,
        ..HitMap::default()
    };
    if r.current_id.depth() > 2 || r.coordinate.is_general() {
        map.push(Rect::new(0.0, ROW_H, 500.0, ROW_H), Hit::Parent);
    }
    for n in 0..r.active_list_len() {
        map.push(Rect::new(0.0, LIST_TOP + n as f32 * ROW_H, 500.0, ROW_H), Hit::Row(n));
    }
    if r.current_id.depth() == 2 {
        let top = LIST_TOP + TWO_BUTTONS as f32 * ROW_H;
        map.push(
            Rect::new(100.0, top, 50.0, ROW_H),
            Hit::Target { row: TWO_BUTTONS, target: 0 },
        );
        map.push(
            Rect::new(200.0, top, 50.0, ROW_H),
            Hit::Target { row: TWO_BUTTONS, target: 1 },
        );
    }
    r.hit_map = map;
}

/// The edited field drawn on row `n`, one stop every 10 px from x = 100.
fn field_on_row(r: &mut AppRenderer, n: usize) {
    let top = LIST_TOP + n as f32 * ROW_H;
    let len = r.input_buffer.len();
    r.hit_map.field = vec![CaretLine {
        top,
        bottom: top + ROW_H,
        stops: (0..=len).map(|b| (b, 100.0 + 10.0 * b as f32)).collect(),
    }];
}

fn renderer_at(row: usize) -> (AppRenderer, Log) {
    let log: Log = Arc::new(Mutex::new(Vec::new()));
    let mut r = AppRenderer::new();
    registry::register_provider(
        &mut r,
        Box::new(Desk {
            path: "/".to_owned(),
            log: Arc::clone(&log),
            interactive: false,
        }),
    );
    r.current_id = id(&[0, row]);
    list::create_list_current_layer(&mut r);
    r.sync_list_index_from_current_id();
    frame(&mut r);
    (r, log)
}

fn logged(log: &Log) -> Vec<String> {
    log.lock().unwrap().clone()
}

fn click(r: &mut AppRenderer, x: f32, y: f32) {
    pointer::button_down(r, Button::Left, x, y, 1, false);
    pointer::button_up(r, Button::Left, x, y);
}

fn right_drag(r: &mut AppRenderer, from: (f32, f32), to: (f32, f32)) {
    pointer::button_down(r, Button::Right, from.0, from.1, 1, false);
    pointer::motion(r, to.0, to.1);
    pointer::button_up(r, Button::Right, to.0, to.1);
}

/// Click the input row, then lay the field out as the next frame does.
fn start_editing(r: &mut AppRenderer, row: usize, x: f32) {
    click(r, x, row_y(row));
    assert_eq!(r.coordinate, Coordinate::Insert, "a click on a field edits it");
    frame(r);
    field_on_row(r, row);
    pointer::after_frame(r);
}

// ---- The hit map ------------------------------------------------------------

#[test]
fn a_target_wins_over_its_row_and_the_field_over_both() {
    let (mut r, _) = renderer_at(PLAIN);
    let y = row_y(TWO_BUTTONS);
    assert_eq!(r.hit_map.hit(10.0, y), Some(Hit::Row(TWO_BUTTONS)));
    assert_eq!(
        r.hit_map.hit(210.0, y),
        Some(Hit::Target { row: TWO_BUTTONS, target: 1 })
    );
    r.input_buffer = "abc".to_owned();
    field_on_row(&mut r, TWO_BUTTONS);
    assert_eq!(r.hit_map.hit(210.0, y), Some(Hit::Field));
    assert_eq!(r.hit_map.hit(10.0, 1000.0), None);
}

#[test]
fn the_caret_goes_to_the_nearest_stop_on_the_line_clicked() {
    let (mut r, _) = renderer_at(PLAIN);
    r.input_buffer = "hello".to_owned();
    field_on_row(&mut r, 0);
    let y = row_y(0);
    assert_eq!(r.hit_map.caret_at(0.0, y), Some(0));
    assert_eq!(r.hit_map.caret_at(124.0, y), Some(2));
    assert_eq!(r.hit_map.caret_at(126.0, y), Some(3));
    assert_eq!(r.hit_map.caret_at(900.0, y), Some(5));
    // Above or below the field: its first or last line.
    assert_eq!(r.hit_map.caret_at(126.0, 0.0), Some(3));
    assert_eq!(r.hit_map.caret_at(126.0, 900.0), Some(3));
}

#[test]
fn a_dashboard_cell_is_clamped_to_the_grid() {
    let map = HitMap {
        grid: Some(CellGrid {
            top: 20.0,
            cell_w: 10.0,
            cell_h: 20.0,
            cols: 80,
            rows: 24,
        }),
        ..HitMap::default()
    };
    assert_eq!(map.cell_at(35.0, 65.0), Some((3, 2)));
    assert_eq!(map.cell_at(9000.0, 9000.0), Some((79, 23)));
    assert_eq!(map.hit(5.0, 10.0), None, "the header is no cell");
    assert_eq!(map.hit(5.0, 30.0), Some(Hit::Dashboard));
}

// ---- A left click acts at once ----------------------------------------------

#[test]
fn a_click_on_a_checkbox_toggles_it() {
    let (mut r, log) = renderer_at(PLAIN);
    click(&mut r, 10.0, row_y(CHECKBOX));
    assert_eq!(r.list_index, CHECKBOX);
    assert_eq!(logged(&log), vec!["checkbox Wifi true"]);
}

#[test]
fn a_click_on_a_button_presses_it() {
    let (mut r, log) = renderer_at(PLAIN);
    click(&mut r, 10.0, row_y(BUTTON));
    assert_eq!(logged(&log), vec!["button do_it"]);
}

#[test]
fn a_click_on_a_folder_opens_it() {
    let (mut r, _) = renderer_at(PLAIN);
    click(&mut r, 10.0, row_y(FOLDER));
    assert_eq!(r.current_id, id(&[0, FOLDER, 0]));
    assert_eq!(r.total_list.len(), 2);
}

#[test]
fn a_click_on_the_parent_line_goes_to_the_parent() {
    let (mut r, _) = renderer_at(PLAIN);
    click(&mut r, 10.0, row_y(FOLDER));
    frame(&mut r);
    click(&mut r, 10.0, ROW_H * 1.5);
    assert_eq!(r.current_id, id(&[0, FOLDER]));
}

#[test]
fn a_click_on_one_of_several_targets_uses_that_one() {
    let (mut r, log) = renderer_at(PLAIN);
    click(&mut r, 210.0, row_y(TWO_BUTTONS));
    assert_eq!(logged(&log), vec!["button second"]);
    assert_eq!(r.list_index, TWO_BUTTONS);
    assert_eq!(r.coordinate, Coordinate::General, "back where the click was");
}

#[test]
fn a_click_beside_several_targets_only_selects_the_row() {
    let (mut r, log) = renderer_at(PLAIN);
    click(&mut r, 10.0, row_y(TWO_BUTTONS));
    assert_eq!(r.list_index, TWO_BUTTONS);
    assert!(logged(&log).is_empty(), "no target used: {:?}", logged(&log));
}

#[test]
fn a_press_and_a_release_on_different_rows_is_no_click() {
    let (mut r, log) = renderer_at(PLAIN);
    pointer::button_down(&mut r, Button::Left, 10.0, row_y(BUTTON), 1, false);
    pointer::button_up(&mut r, Button::Left, 10.0, row_y(PLAIN));
    assert_eq!(r.list_index, PLAIN);
    assert!(logged(&log).is_empty());
}

// ---- Fields -----------------------------------------------------------------

#[test]
fn a_click_on_a_field_edits_it_with_the_caret_where_it_was() {
    let (mut r, _) = renderer_at(PLAIN);
    start_editing(&mut r, INPUT, 131.0);
    assert_eq!(r.input_buffer, "hello world");
    assert_eq!(r.cursor_position, 3);
}

#[test]
fn a_drag_in_the_field_selects() {
    let (mut r, _) = renderer_at(PLAIN);
    start_editing(&mut r, INPUT, 100.0);
    let y = row_y(INPUT);
    pointer::button_down(&mut r, Button::Left, 120.0, y, 1, false);
    pointer::motion(&mut r, 150.0, y);
    pointer::button_up(&mut r, Button::Left, 150.0, y);
    assert_eq!(r.selection_anchor, Some(2));
    assert_eq!(r.cursor_position, 5);
    assert_eq!(r.coordinate, Coordinate::Insert);
}

#[test]
fn a_shift_click_in_the_field_grows_the_selection() {
    let (mut r, _) = renderer_at(PLAIN);
    start_editing(&mut r, INPUT, 120.0);
    let y = row_y(INPUT);
    pointer::button_down(&mut r, Button::Left, 170.0, y, 1, true);
    pointer::button_up(&mut r, Button::Left, 170.0, y);
    assert_eq!(r.selection_anchor, Some(2));
    assert_eq!(r.cursor_position, 7);
}

#[test]
fn a_double_click_in_the_field_is_enter() {
    let (mut r, log) = renderer_at(PLAIN);
    start_editing(&mut r, INPUT, 100.0);
    r.input_buffer = "hello there".to_owned();
    field_on_row(&mut r, INPUT);
    let y = row_y(INPUT);
    pointer::button_down(&mut r, Button::Left, 120.0, y, 1, false);
    pointer::button_up(&mut r, Button::Left, 120.0, y);
    pointer::button_down(&mut r, Button::Left, 120.0, y, 2, false);
    pointer::button_up(&mut r, Button::Left, 120.0, y);
    assert_eq!(r.coordinate, Coordinate::General);
    assert_eq!(logged(&log), vec!["commit hello world -> hello there"]);
}

#[test]
fn a_click_on_nothing_cancels_the_edit() {
    let (mut r, log) = renderer_at(PLAIN);
    start_editing(&mut r, INPUT, 100.0);
    r.input_buffer = "thrown away".to_owned();
    click(&mut r, 10.0, 1000.0);
    assert_eq!(r.coordinate, Coordinate::General);
    assert!(logged(&log).is_empty(), "nothing committed: {:?}", logged(&log));
}

#[test]
fn a_right_click_cancels_the_edit() {
    let (mut r, log) = renderer_at(PLAIN);
    start_editing(&mut r, INPUT, 100.0);
    r.input_buffer = "thrown away".to_owned();
    let y = row_y(INPUT);
    right_drag(&mut r, (120.0, y), (121.0, y));
    assert_eq!(r.coordinate, Coordinate::General);
    assert!(logged(&log).is_empty(), "nothing committed: {:?}", logged(&log));
}

#[test]
fn a_click_on_another_field_commits_and_goes_on_editing_there() {
    let (mut r, log) = renderer_at(PLAIN);
    start_editing(&mut r, INPUT, 100.0);
    r.input_buffer = "kept".to_owned();
    click(&mut r, 120.0, row_y(OTHER_INPUT));
    assert_eq!(logged(&log), vec!["commit hello world -> kept"]);
    assert_eq!(r.coordinate, Coordinate::Insert);
    assert_eq!(r.list_index, OTHER_INPUT);
    assert_eq!(r.input_buffer, "other");
    field_on_row(&mut r, OTHER_INPUT);
    pointer::after_frame(&mut r);
    assert_eq!(r.cursor_position, 2, "the caret where the click was");
}

#[test]
fn a_click_on_another_row_commits_then_uses_it() {
    let (mut r, log) = renderer_at(PLAIN);
    start_editing(&mut r, INPUT, 100.0);
    r.input_buffer = "kept".to_owned();
    click(&mut r, 10.0, row_y(BUTTON));
    assert_eq!(
        logged(&log),
        vec!["commit hello world -> kept", "button do_it"]
    );
    assert_eq!(r.coordinate, Coordinate::General);
}

// ---- Back and forward ---------------------------------------------------------

#[test]
fn a_right_button_swipe_left_goes_to_the_parent() {
    let (mut r, _) = renderer_at(PLAIN);
    click(&mut r, 10.0, row_y(FOLDER));
    frame(&mut r);
    right_drag(&mut r, (300.0, row_y(0)), (200.0, row_y(0) + 10.0));
    assert_eq!(r.current_id, id(&[0, FOLDER]));
}

#[test]
fn a_right_button_swipe_right_opens_the_row_it_started_on() {
    let (mut r, _) = renderer_at(PLAIN);
    right_drag(&mut r, (100.0, row_y(FOLDER)), (200.0, row_y(FOLDER)));
    assert_eq!(r.current_id, id(&[0, FOLDER, 0]));
}

#[test]
fn a_short_or_steep_right_drag_does_nothing() {
    let (mut r, log) = renderer_at(PLAIN);
    right_drag(&mut r, (100.0, row_y(FOLDER)), (130.0, row_y(FOLDER)));
    right_drag(&mut r, (100.0, row_y(0)), (160.0, row_y(5)));
    assert_eq!(r.current_id, id(&[0, PLAIN]));
    assert!(logged(&log).is_empty());
}

#[test]
fn a_right_click_is_escape_out_of_a_list_drawn_over_the_level() {
    let (mut r, _) = renderer_at(PLAIN);
    r.coordinate = Coordinate::Meta;
    right_drag(&mut r, (100.0, row_y(0)), (100.0, row_y(0)));
    assert_ne!(r.coordinate, Coordinate::Meta);
}

#[test]
fn the_side_buttons_are_back_and_forward() {
    let (mut r, _) = renderer_at(FOLDER);
    pointer::button_down(&mut r, Button::Forward, 0.0, 0.0, 1, false);
    assert_eq!(r.current_id, id(&[0, FOLDER, 0]));
    pointer::button_down(&mut r, Button::Back, 0.0, 0.0, 1, false);
    assert_eq!(r.current_id, id(&[0, FOLDER]));
}

// ---- The wheel and the touchpad -----------------------------------------------

#[test]
fn the_wheel_moves_the_selection_a_row_a_notch() {
    let (mut r, _) = renderer_at(BUTTON);
    pointer::wheel(&mut r, 0.0, -1.0, 0);
    assert_eq!(r.list_index, FOLDER);
    // A touchpad's small steps add up to a row.
    pointer::wheel(&mut r, 0.0, -0.5, 10);
    assert_eq!(r.list_index, FOLDER);
    pointer::wheel(&mut r, 0.0, -0.5, 20);
    assert_eq!(r.list_index, INPUT);
    pointer::wheel(&mut r, 0.0, 2.0, 30);
    assert_eq!(r.list_index, BUTTON);
}

#[test]
fn two_fingers_sideways_go_back_once_a_gesture() {
    let (mut r, _) = renderer_at(PLAIN);
    click(&mut r, 10.0, row_y(FOLDER));
    assert_eq!(r.current_id.depth(), 3);
    // Two fingers moving right scroll left: back.
    pointer::wheel(&mut r, -2.0, 0.1, 1000);
    assert_eq!(r.current_id.depth(), 3, "not far enough yet");
    pointer::wheel(&mut r, -3.0, 0.0, 1010);
    assert_eq!(r.current_id, id(&[0, FOLDER]));
    // The rest of the same gesture does nothing more.
    pointer::wheel(&mut r, -9.0, 0.0, 1020);
    assert_eq!(r.current_id, id(&[0, FOLDER]));
    // A new gesture, after a pause: forward, into the folder again.
    pointer::wheel(&mut r, 5.0, 0.0, 2000);
    assert_eq!(r.current_id, id(&[0, FOLDER, 0]));
}

#[test]
fn a_vertical_scroll_drifting_sideways_is_no_swipe() {
    let (mut r, _) = renderer_at(PLAIN);
    for t in 0..10 {
        pointer::wheel(&mut r, -0.6, -1.0, t * 10);
    }
    assert_eq!(r.current_id.depth(), 2);
}

// ---- The touch screen -----------------------------------------------------------

#[test]
fn a_tap_is_a_click() {
    let (mut r, log) = renderer_at(PLAIN);
    pointer::finger_down(&mut r, 1, 10.0, row_y(BUTTON), 0);
    pointer::finger_up(&mut r, 1, 12.0, row_y(BUTTON), 80);
    assert_eq!(logged(&log), vec!["button do_it"]);
}

#[test]
fn a_finger_dragged_up_brings_the_rows_below() {
    let (mut r, log) = renderer_at(CHECKBOX);
    pointer::finger_down(&mut r, 1, 10.0, 200.0, 0);
    pointer::finger_motion(&mut r, 1, 10.0, 200.0 - 2.5 * ROW_H);
    pointer::finger_up(&mut r, 1, 10.0, 200.0 - 2.5 * ROW_H, 300);
    assert_eq!(r.list_index, FOLDER);
    assert!(logged(&log).is_empty(), "a drag is no tap");
}

#[test]
fn a_finger_swipe_left_goes_to_the_parent() {
    let (mut r, _) = renderer_at(PLAIN);
    click(&mut r, 10.0, row_y(FOLDER));
    frame(&mut r);
    pointer::finger_down(&mut r, 1, 300.0, row_y(0), 0);
    pointer::finger_motion(&mut r, 1, 200.0, row_y(0));
    pointer::finger_up(&mut r, 1, 200.0, row_y(0), 200);
    assert_eq!(r.current_id, id(&[0, FOLDER]));
}

#[test]
fn a_double_tap_in_the_field_is_enter() {
    let (mut r, log) = renderer_at(PLAIN);
    start_editing(&mut r, INPUT, 100.0);
    r.input_buffer = "tapped".to_owned();
    field_on_row(&mut r, INPUT);
    let y = row_y(INPUT);
    pointer::finger_down(&mut r, 1, 120.0, y, 1000);
    pointer::finger_up(&mut r, 1, 120.0, y, 1050);
    assert_eq!(r.coordinate, Coordinate::Insert);
    assert_eq!(r.cursor_position, 2);
    pointer::finger_down(&mut r, 2, 121.0, y, 1200);
    pointer::finger_up(&mut r, 2, 121.0, y, 1250);
    assert_eq!(r.coordinate, Coordinate::General);
    assert_eq!(logged(&log), vec!["commit hello world -> tapped"]);
}

// ---- The terminal's dashboard ---------------------------------------------------

fn dashboard() -> (AppRenderer, Log) {
    let log: Log = Arc::new(Mutex::new(Vec::new()));
    let mut r = AppRenderer::new();
    registry::register_provider(
        &mut r,
        Box::new(Desk {
            path: "/".to_owned(),
            log: Arc::clone(&log),
            interactive: true,
        }),
    );
    r.current_id = id(&[0, PLAIN]);
    list::create_list_current_layer(&mut r);
    r.sync_list_index_from_current_id();
    r.previous_coordinate = Coordinate::General;
    r.coordinate = Coordinate::Dashboard;
    let mut map = HitMap {
        line_height: ROW_H,
        grid: Some(CellGrid {
            top: ROW_H,
            cell_w: 10.0,
            cell_h: ROW_H,
            cols: 80,
            rows: 24,
        }),
        ..HitMap::default()
    };
    map.push(Rect::new(0.0, 0.0, 800.0, ROW_H), Hit::Header);
    r.hit_map = map;
    (r, log)
}

#[test]
fn the_terminal_gets_the_left_button_and_the_wheel_at_a_cell() {
    let (mut r, log) = dashboard();
    pointer::motion(&mut r, 35.0, 65.0);
    pointer::button_down(&mut r, Button::Left, 35.0, 65.0, 1, false);
    pointer::button_up(&mut r, Button::Left, 35.0, 65.0);
    pointer::wheel(&mut r, 0.0, 1.0, 0);
    crate::provider::forward_dashboard_mouse(&mut r);
    assert_eq!(
        logged(&log),
        vec![
            "mouse Motion None 3,2",
            "mouse Press Some(Left) 3,2",
            "mouse Release Some(Left) 3,2",
            "mouse WheelUp None 3,2",
        ]
    );
    assert_eq!(r.coordinate, Coordinate::Dashboard);
}

#[test]
fn a_click_on_the_header_leaves_the_terminal() {
    let (mut r, log) = dashboard();
    click(&mut r, 50.0, 5.0);
    assert_ne!(r.coordinate, Coordinate::Dashboard);
    assert_eq!(logged(&log), vec!["leave dashboard"]);
}

#[test]
fn the_back_gesture_leaves_the_terminal_and_its_right_button_is_the_apps() {
    let (mut r, log) = dashboard();
    right_drag(&mut r, (400.0, 100.0), (300.0, 100.0));
    crate::provider::forward_dashboard_mouse(&mut r);
    assert_ne!(r.coordinate, Coordinate::Dashboard);
    assert_eq!(logged(&log), vec!["leave dashboard"], "no right button sent");
}

// ---- A screen reader's or voice control's "activate" ---------------------------

#[test]
fn activate_does_what_a_click_on_the_selected_row_does() {
    let (mut r, log) = renderer_at(BUTTON);
    pointer::activate(&mut r);
    assert_eq!(logged(&log), vec!["button do_it"]);
    let (mut r, _) = renderer_at(FOLDER);
    pointer::activate(&mut r);
    assert_eq!(r.current_id, id(&[0, FOLDER, 0]));
}

#[test]
fn activate_leaves_a_field_being_edited_alone() {
    let (mut r, log) = renderer_at(PLAIN);
    start_editing(&mut r, INPUT, 100.0);
    pointer::activate(&mut r);
    assert_eq!(r.coordinate, Coordinate::Insert);
    assert!(logged(&log).is_empty());
}

// ---- What a screen magnifier follows ----------------------------------------------

#[test]
fn the_focus_rect_is_the_selected_row() {
    let (r, _) = renderer_at(FOLDER);
    let rect = pointer::focus_rect(&r).expect("the selected row is drawn");
    assert_eq!(rect.y, LIST_TOP + FOLDER as f32 * ROW_H);
    assert_eq!(rect.h, ROW_H);
}

#[test]
fn the_focus_rect_follows_the_caret_while_typing() {
    let (mut r, _) = renderer_at(PLAIN);
    start_editing(&mut r, INPUT, 131.0);
    assert_eq!(r.cursor_position, 3);
    let rect = pointer::focus_rect(&r).expect("the caret is drawn");
    assert_eq!(rect.x, 130.0, "at the caret's stop");
    assert_eq!(rect.y, LIST_TOP + INPUT as f32 * ROW_H);
    assert!(rect.w <= 4.0, "a caret, not the whole row: {rect:?}");
}

#[test]
fn the_focus_rect_is_the_target_in_element_mode() {
    let (mut r, _) = renderer_at(TWO_BUTTONS);
    crate::shortcuts::dispatch_key(&mut r, Some(sdl3::keyboard::Keycode::Right), sdl3::keyboard::Mod::NOMOD);
    crate::shortcuts::dispatch_key(&mut r, Some(sdl3::keyboard::Keycode::Right), sdl3::keyboard::Mod::NOMOD);
    assert_eq!(r.coordinate, Coordinate::InElement);
    assert_eq!(r.nav_index, 1);
    let rect = pointer::focus_rect(&r).expect("the target is drawn");
    assert_eq!(rect.x, 200.0, "the second button's rect");
}
