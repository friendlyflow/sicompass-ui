//! The mouse, the touchpad and the touch screen.
//!
//! The pointer chooses a row and then presses a key. Everything a key would do goes
//! through [`crate::events::dispatch_key`], so the shortcut table, the timeline,
//! the tabs and what is said stay exactly what the keys make them.
//!
//! - A left click selects the row under it and does at once what the row is for
//!   ([`crate::shortcuts::general_click`]). The parent line above the list goes to
//!   the parent.
//! - A right click, and a click on nothing, is Escape.
//! - A swipe with the right button held (or the mouse's side buttons, two fingers
//!   sideways on a touchpad, one finger on a touch screen) is Left or Right. Out of
//!   a list drawn over a level, back is Escape.
//! - The wheel is Up and Down.
//! - In a field being edited, a click places the caret, a drag selects and a
//!   double click is Enter. A click on another row commits the field (Enter) and
//!   then does what that click does, so a click on another field goes on editing
//!   there. A click on nothing cancels the edit, as Escape does.
//! - On the terminal's dashboard the left button, the middle one and the wheel
//!   are the program's. The right button is still the app's, and a click on the
//!   header line above the grid leaves, as a double Ctrl+C does.
//!
//! What is under the pointer comes from the [`HitMap`] the last frame drew, in
//! physical pixels. The view converts SDL's logical points before calling in.

use crate::app_state::{AppRenderer, Coordinate};
use crate::events;
use crate::handlers;
use crate::shortcuts::Click;
use sdl3::keyboard::{Keycode, Mod};

/// A swipe is at least this long, in logical points, and mostly sideways.
pub const SWIPE_MIN_PT: f32 = 48.0;
/// How much longer than tall a swipe's travel must be.
pub const SWIPE_RATIO: f32 = 1.5;
/// Two fingers sideways on a touchpad: how far, in wheel units, before it fires.
pub const SIDEWAYS_WHEEL_MIN: f32 = 4.0;
/// A pause in wheel events this long ends a touchpad gesture.
pub const WHEEL_GESTURE_GAP_MS: u64 = 300;
/// A finger travels this far, in points, before it is a drag and not a tap.
pub const TAP_SLOP_PT: f32 = 10.0;
/// A finger held this long without moving is a long press.
pub const LONG_PRESS_MS: u64 = 500;
/// A second tap this soon after the first, in the same place, is a double tap.
pub const DOUBLE_TAP_MS: u64 = 400;

/// An axis-aligned rectangle in physical pixels.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct Rect {
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
}

impl Rect {
    pub fn new(x: f32, y: f32, w: f32, h: f32) -> Self {
        Self { x, y, w, h }
    }

    pub fn contains(&self, x: f32, y: f32) -> bool {
        x >= self.x && x < self.x + self.w && y >= self.y && y < self.y + self.h
    }
}

/// What is under the pointer.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Hit {
    /// The parent line above the list: a click goes to the parent.
    Parent,
    /// Row `n` of the active list, counted as `list_index` counts.
    Row(usize),
    /// Target `target` of row `row`: a link, a button, an image or a video.
    Target { row: usize, target: usize },
    /// The text field being edited or typed in.
    Field,
    /// The interactive dashboard's grid of cells.
    Dashboard,
    /// The header line of a full-screen view: a click leaves it.
    Header,
    /// Text being read (scroll mode): a click there does nothing, the wheel and a
    /// finger scroll it.
    Page,
}

/// One drawn line of the text field, and where a caret can go on it.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct CaretLine {
    pub top: f32,
    pub bottom: f32,
    /// `(byte in the field's buffer, x of a caret before it)`, left to right, the
    /// line's end included.
    pub stops: Vec<(usize, f32)>,
}

/// The interactive dashboard's cells, as the frame drew them.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct CellGrid {
    pub top: f32,
    pub cell_w: f32,
    pub cell_h: f32,
    pub cols: u16,
    pub rows: u16,
}

/// Where everything was on the last frame, in physical pixels. Filled by the view
/// every frame, read by the pointer between frames.
#[derive(Debug, Clone, PartialEq)]
pub struct HitMap {
    /// Physical pixels per logical point, the factor the view multiplies SDL's
    /// coordinates by. Also what turns [`SWIPE_MIN_PT`] into pixels.
    pub px_per_pt: f32,
    /// One line of the list, for a finger's drag and the wheel's pixels.
    pub line_height: f32,
    /// Later regions are drawn over earlier ones, so a target wins over its row.
    pub regions: Vec<(Rect, Hit)>,
    /// The text field's lines, empty when no field takes the caret.
    pub field: Vec<CaretLine>,
    pub grid: Option<CellGrid>,
}

impl Default for HitMap {
    fn default() -> Self {
        Self {
            px_per_pt: 1.0,
            line_height: 0.0,
            regions: Vec::new(),
            field: Vec::new(),
            grid: None,
        }
    }
}

impl HitMap {
    /// Start a new frame: everything goes but the scale.
    pub fn clear(&mut self) {
        self.regions.clear();
        self.field.clear();
        self.grid = None;
    }

    pub fn push(&mut self, rect: Rect, hit: Hit) {
        if rect.w > 0.0 && rect.h > 0.0 {
            self.regions.push((rect, hit));
        }
    }

    /// What is at `(x, y)`.
    pub fn hit(&self, x: f32, y: f32) -> Option<Hit> {
        if let Some(grid) = self.grid
            && y >= grid.top
        {
            return Some(Hit::Dashboard);
        }
        if self.field.iter().any(|l| y >= l.top && y < l.bottom) {
            return Some(Hit::Field);
        }
        self.regions
            .iter()
            .rev()
            .find(|(r, _)| r.contains(x, y))
            .map(|&(_, hit)| hit)
    }

    /// The byte a caret placed at `(x, y)` lands before: on the field's line at
    /// that height (the first or last above or below it), at the nearest stop.
    pub fn caret_at(&self, x: f32, y: f32) -> Option<usize> {
        let line = self
            .field
            .iter()
            .find(|l| y >= l.top && y < l.bottom)
            .or_else(|| match self.field.first() {
                Some(first) if y < first.top => Some(first),
                _ => self.field.last(),
            })?;
        line.stops
            .iter()
            .min_by(|a, b| (a.1 - x).abs().total_cmp(&(b.1 - x).abs()))
            .map(|&(byte, _)| byte)
    }

    /// The dashboard cell at `(x, y)`, 0-based, clamped to the grid.
    pub fn cell_at(&self, x: f32, y: f32) -> Option<(u16, u16)> {
        let g = self.grid?;
        let col = (x / g.cell_w.max(1.0)).floor().max(0.0) as u16;
        let row = ((y - g.top) / g.cell_h.max(1.0)).floor().max(0.0) as u16;
        Some((
            col.min(g.cols.saturating_sub(1)),
            row.min(g.rows.saturating_sub(1)),
        ))
    }

    fn swipe_min(&self) -> f32 {
        SWIPE_MIN_PT * self.px_per_pt
    }
}

/// A mouse button, as the pointer sees it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Button {
    Left,
    Middle,
    Right,
    /// The side button browsers use for back.
    Back,
    /// The side button browsers use for forward.
    Forward,
}

impl Button {
    pub fn from_sdl(b: sdl3::mouse::MouseButton) -> Option<Self> {
        use sdl3::mouse::MouseButton as M;
        Some(match b {
            M::Left => Self::Left,
            M::Middle => Self::Middle,
            M::Right => Self::Right,
            M::X1 => Self::Back,
            M::X2 => Self::Forward,
            M::Unknown => return None,
        })
    }
}

/// What the cursor should look like over what it is on.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum CursorShape {
    #[default]
    Arrow,
    /// Over something a click uses.
    Hand,
    /// Over a text field.
    Text,
}

/// What the dashboard is told about the mouse.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DashboardMouseKind {
    Press,
    Release,
    Motion,
    WheelUp,
    WheelDown,
}

#[derive(Debug, Clone, Copy, PartialEq)]
struct Press {
    x: f32,
    y: f32,
    hit: Option<Hit>,
    /// The caret's other end while a left drag selects in a field.
    anchor: Option<usize>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum TouchDrag {
    Undecided,
    Vertical,
    Sideways,
}

#[derive(Debug, Clone, Copy, PartialEq)]
struct Touch {
    id: u64,
    x0: f32,
    y0: f32,
    x: f32,
    y: f32,
    t0: u64,
    drag: TouchDrag,
    /// Rows the drag has moved so far.
    rows: i32,
}

#[derive(Debug, Clone, Copy, PartialEq, Default)]
struct Wheel {
    /// Vertical travel not yet spent on a row.
    y: f32,
    /// This gesture's sideways and vertical travel.
    gesture_x: f32,
    gesture_y: f32,
    /// The gesture already went Left or Right.
    fired: bool,
    last_ms: u64,
}

/// The pointer's state between events, on [`AppRenderer::pointer`].
#[derive(Debug, Clone, Default)]
pub struct PointerState {
    left: Option<Press>,
    right: Option<Press>,
    touch: Option<Touch>,
    wheel: Wheel,
    /// A click that started editing a field: where, so the caret goes there
    /// once the next frame has laid the field out.
    pending_caret: Option<(f32, f32)>,
    /// Where the pointer last was, to keep the cursor's shape true after a frame
    /// moved what is under it.
    last: Option<(f32, f32)>,
    /// The last tap on a touch screen: when and where, for a double tap.
    last_tap: Option<(u64, f32, f32)>,
    /// The shape the cursor should have. The view sets it when it changes.
    pub cursor: CursorShape,
    /// Dashboard mouse events for the view to forward, in order.
    pub dashboard: Vec<(DashboardMouseKind, Option<Button>, u16, u16)>,
}

// ---- What the pointer does ---------------------------------------------------

fn press(r: &mut AppRenderer, k: Keycode) -> bool {
    events::dispatch_key(r, Some(k), Mod::NOMOD)
}

/// The modes where the caret is in a text field the pointer can place it in.
fn field_mode(r: &AppRenderer) -> bool {
    matches!(
        r.coordinate.base(),
        Coordinate::Insert
            | Coordinate::Normal
            | Coordinate::Visual
            | Coordinate::SimpleSearch
            | Coordinate::ExtendedSearch
            | Coordinate::Command
            | Coordinate::InputSearch
            | Coordinate::Meta
            | Coordinate::TabSwitcher
    )
}

fn editing(r: &AppRenderer) -> bool {
    matches!(
        r.coordinate,
        Coordinate::Insert | Coordinate::Normal | Coordinate::Visual | Coordinate::InputSearch
    )
}

/// The lists the app draws over a level, where Enter chooses a row.
fn overlay(r: &AppRenderer) -> bool {
    matches!(
        r.coordinate.base(),
        Coordinate::SimpleSearch
            | Coordinate::ExtendedSearch
            | Coordinate::Command
            | Coordinate::Meta
            | Coordinate::Manual
            | Coordinate::TimelineView
            | Coordinate::ConfirmCloseTab
            | Coordinate::TabSwitcher
    )
}

fn interactive_dashboard(r: &AppRenderer) -> bool {
    r.coordinate == Coordinate::Dashboard && handlers::active_dashboard_is_interactive(r)
}

/// Whether the mouse events gathered for the dashboard still have one to go to.
pub fn dashboard_takes_mouse(r: &AppRenderer) -> bool {
    interactive_dashboard(r)
}

/// Ctrl, Shift and Alt as they are held now.
pub fn modifiers_held() -> (bool, bool, bool) {
    // SAFETY: reads SDL's keyboard state, on the thread that pumps its events.
    let mods = unsafe { sdl3::sys::keyboard::SDL_GetModState() }.0;
    let held = |m: sdl3::sys::keycode::SDL_Keymod| mods & m.0 != 0;
    use sdl3::sys::keycode::{SDL_KMOD_ALT, SDL_KMOD_CTRL, SDL_KMOD_SHIFT};
    (held(SDL_KMOD_CTRL), held(SDL_KMOD_SHIFT), held(SDL_KMOD_ALT))
}

/// Move the selection to row `n` of the active list, the way Up and Down move it.
/// `false` when there is no such row.
pub fn select_row(r: &mut AppRenderer, n: usize) -> bool {
    if n >= r.active_list_len() {
        return false;
    }
    events::with_tab_sync(r, |r| {
        r.error_message.clear();
        if r.coordinate.is_general() {
            r.previous_id = r.current_id.clone();
        }
        r.list_index = n;
        if handlers::list_row_is_cursor_id(r) {
            r.sync_current_id_from_list();
        }
        if r.coordinate.is_general() {
            crate::list::create_list_current_layer(r);
        }
        r.needs_redraw = true;
    });
    true
}

/// What a left click on row `n` does, at `(x, y)`.
fn click_row(r: &mut AppRenderer, n: usize, x: f32, y: f32) -> bool {
    if r.coordinate == Coordinate::InElement {
        press(r, Keycode::Escape);
    }
    if (r.coordinate.is_general() || overlay(r)) && select_row(r, n) {
        return use_selected(r, Some((x, y)));
    }
    false
}

/// Do what a click does to the selected row. `at` is where the click was, for the
/// caret of a field it starts editing.
fn use_selected(r: &mut AppRenderer, at: Option<(f32, f32)>) -> bool {
    // A launcher's own keymap uses a row with Enter, and so does a list drawn over
    // a level.
    if (r.launcher_mode && r.coordinate.is_general()) || overlay(r) {
        return press(r, Keycode::Return);
    }
    if !r.coordinate.is_general() {
        return false;
    }
    match crate::shortcuts::general_click(r) {
        Click::Press(Keycode::I) => {
            r.pointer.pending_caret = at;
            press(r, Keycode::I)
        }
        Click::Press(k) => press(r, k),
        Click::Open => {
            events::with_tab_sync(r, handlers::handle_right);
            false
        }
        Click::Nothing => {
            r.speak_current_element();
            false
        }
    }
}

/// A screen reader's or voice control's "activate" (AccessKit's Click on the
/// focused row): what a click on the selected row does. In element mode and on a
/// picture or a video, Enter. Nothing while a field is edited.
pub fn activate(r: &mut AppRenderer) -> bool {
    match r.coordinate {
        Coordinate::InElement => press(r, Keycode::Return),
        Coordinate::Dashboard if !interactive_dashboard(r) => press(r, Keycode::Return),
        _ if editing(r) => false,
        _ => use_selected(r, None),
    }
}

/// Leave a full-screen view: the terminal's as a double Ctrl+C does, any other
/// as Escape does.
fn leave_dashboard(r: &mut AppRenderer) -> bool {
    if interactive_dashboard(r) {
        events::with_tab_sync(r, handlers::handle_dashboard_leave);
        false
    } else {
        press(r, Keycode::Escape)
    }
}

/// A left click on nothing, and a right click: Escape. On the terminal's dashboard
/// Escape is the program's, so there the right click leaves.
fn escape(r: &mut AppRenderer) -> bool {
    if r.coordinate == Coordinate::Dashboard {
        return leave_dashboard(r);
    }
    press(r, Keycode::Escape)
}

/// A left click: press and release on the same thing (`None`: on nothing).
fn click(r: &mut AppRenderer, hit: Option<Hit>, x: f32, y: f32) -> bool {
    let Some(hit) = hit else {
        // On nothing: Escape, which also cancels an edit.
        return escape(r);
    };
    // A click on another row commits the field first, then does what it does.
    if editing(r) && hit != Hit::Field {
        let quit = press(r, Keycode::Return);
        if quit || editing(r) {
            return quit;
        }
    }
    match hit {
        Hit::Field | Hit::Page => false,
        // The terminal's clicks are the program's. On a picture or a video, Enter
        // (which pauses a video).
        Hit::Dashboard if interactive_dashboard(r) => false,
        Hit::Dashboard => press(r, Keycode::Return),
        Hit::Header => leave_dashboard(r),
        Hit::Parent => {
            if r.coordinate == Coordinate::InElement {
                press(r, Keycode::Escape);
            }
            press(r, Keycode::Left)
        }
        Hit::Row(n) => click_row(r, n, x, y),
        Hit::Target { row, target } => {
            let same_row = r.list_index == row;
            if r.coordinate == Coordinate::InElement && !same_row {
                press(r, Keycode::Escape);
            }
            if r.coordinate != Coordinate::InElement && !select_row(r, row) {
                return false;
            }
            events::with_tab_sync(r, |r| crate::in_element::click_target(r, target));
            false
        }
    }
}

/// Back (a swipe left, the back button, two fingers right on a touchpad): Left in
/// a list, and out of whatever is drawn over it.
pub fn go_back(r: &mut AppRenderer) -> bool {
    // One step, as Escape is: out of the edit (cancelling it) or the row first.
    if editing(r) || r.coordinate == Coordinate::InElement {
        return press(r, Keycode::Escape);
    }
    match r.coordinate.base() {
        Coordinate::General => press(r, Keycode::Left),
        Coordinate::Dashboard => leave_dashboard(r),
        _ => press(r, Keycode::Escape),
    }
}

/// Forward: Right in a list, on the row the swipe started on when it started on
/// one. In a list drawn over a level, Enter on that row. In a field being edited,
/// Enter, keeping the edit (back cancels it).
pub fn go_forward(r: &mut AppRenderer, from: Option<Hit>) -> bool {
    if editing(r) {
        return press(r, Keycode::Return);
    }
    let row = match from {
        Some(Hit::Row(n) | Hit::Target { row: n, .. }) => Some(n),
        _ => None,
    };
    if r.coordinate == Coordinate::InElement && row.is_some_and(|n| n != r.list_index) {
        press(r, Keycode::Escape);
    }
    if let Some(n) = row
        && r.coordinate != Coordinate::InElement
    {
        select_row(r, n);
    }
    if overlay(r) {
        return row.is_some() && press(r, Keycode::Return);
    }
    match r.coordinate.base() {
        Coordinate::General | Coordinate::InElement => press(r, Keycode::Right),
        _ => false,
    }
}

/// Is this travel a swipe, and which way: `Some(true)` back (left), `Some(false)`
/// forward (right).
fn swipe(map: &HitMap, dx: f32, dy: f32) -> Option<bool> {
    (dx.abs() >= map.swipe_min() && dx.abs() > SWIPE_RATIO * dy.abs()).then_some(dx < 0.0)
}

fn shape_at(r: &AppRenderer, x: f32, y: f32) -> CursorShape {
    match r.hit_map.hit(x, y) {
        Some(Hit::Field) if field_mode(r) => CursorShape::Text,
        Some(Hit::Field | Hit::Page) => CursorShape::Arrow,
        Some(Hit::Dashboard) => CursorShape::Text,
        Some(Hit::Parent | Hit::Row(_) | Hit::Target { .. } | Hit::Header) => CursorShape::Hand,
        None => CursorShape::Arrow,
    }
}

fn to_dashboard(r: &mut AppRenderer, kind: DashboardMouseKind, b: Option<Button>, x: f32, y: f32) {
    if let Some((col, row)) = r.hit_map.cell_at(x, y) {
        r.pointer.dashboard.push((kind, b, col, row));
    }
}

// ---- Entry points --------------------------------------------------------------
// Each returns `true` when what it did asks the app to quit, as `dispatch_key`
// does. Coordinates are physical pixels.

pub fn motion(r: &mut AppRenderer, x: f32, y: f32) -> bool {
    r.pointer.last = Some((x, y));
    r.pointer.cursor = shape_at(r, x, y);
    if interactive_dashboard(r) && r.hit_map.grid.is_some() {
        let held = r.pointer.left.map(|_| Button::Left);
        to_dashboard(r, DashboardMouseKind::Motion, held, x, y);
        return false;
    }
    // A left drag from the field selects.
    if let Some(Press {
        anchor: Some(anchor),
        ..
    }) = r.pointer.left
        && let Some(at) = r.hit_map.caret_at(x, y)
    {
        handlers::place_caret(r, at, Some(anchor));
    }
    false
}

pub fn button_down(
    r: &mut AppRenderer,
    b: Button,
    x: f32,
    y: f32,
    clicks: u8,
    shift: bool,
) -> bool {
    r.pointer.last = Some((x, y));
    let hit = r.hit_map.hit(x, y);
    if interactive_dashboard(r)
        && hit == Some(Hit::Dashboard)
        && matches!(b, Button::Left | Button::Middle)
    {
        to_dashboard(r, DashboardMouseKind::Press, Some(b), x, y);
        if b == Button::Left {
            r.pointer.left = Some(Press { x, y, hit, anchor: None });
        }
        return false;
    }
    match b {
        Button::Left => {
            let mut anchor = None;
            if hit == Some(Hit::Field)
                && field_mode(r)
                && let Some(at) = r.hit_map.caret_at(x, y)
            {
                if clicks >= 2 {
                    // A double click in the field is Enter.
                    r.pointer.left = None;
                    return press(r, Keycode::Return);
                } else {
                    let keep = shift.then(|| r.selection_anchor.unwrap_or(r.cursor_position));
                    handlers::place_caret(r, at, keep);
                    anchor = Some(keep.unwrap_or(at));
                }
            }
            r.pointer.left = Some(Press { x, y, hit, anchor });
            false
        }
        Button::Right => {
            r.pointer.right = Some(Press { x, y, hit, anchor: None });
            false
        }
        Button::Back => go_back(r),
        Button::Forward => go_forward(r, None),
        Button::Middle => false,
    }
}

pub fn button_up(r: &mut AppRenderer, b: Button, x: f32, y: f32) -> bool {
    r.pointer.last = Some((x, y));
    if interactive_dashboard(r)
        && matches!(b, Button::Left | Button::Middle)
        && r.pointer.left.is_some_and(|p| p.hit == Some(Hit::Dashboard))
    {
        r.pointer.left = None;
        to_dashboard(r, DashboardMouseKind::Release, Some(b), x, y);
        return false;
    }
    match b {
        Button::Left => {
            let Some(down) = r.pointer.left.take() else {
                return false;
            };
            if down.anchor.is_some() {
                if handlers::has_selection(r) {
                    handlers::announce_pointer_selection(r);
                }
                return false;
            }
            if r.hit_map.hit(x, y) != down.hit {
                return false;
            }
            click(r, down.hit, x, y)
        }
        Button::Right => {
            let Some(down) = r.pointer.right.take() else {
                return false;
            };
            match swipe(&r.hit_map, x - down.x, y - down.y) {
                Some(true) => go_back(r),
                Some(false) => go_forward(r, down.hit),
                // A short drag is no swipe and no click.
                None if (x - down.x).hypot(y - down.y) >= TAP_SLOP_PT * r.hit_map.px_per_pt => {
                    false
                }
                None => escape(r),
            }
        }
        _ => false,
    }
}

/// The wheel, or two fingers on a touchpad. `dy` above zero is away from the user.
pub fn wheel(r: &mut AppRenderer, dx: f32, dy: f32, now_ms: u64) -> bool {
    let mut quit = false;
    let w = &mut r.pointer.wheel;
    if now_ms.saturating_sub(w.last_ms) > WHEEL_GESTURE_GAP_MS {
        w.gesture_x = 0.0;
        w.gesture_y = 0.0;
        w.fired = false;
    }
    w.last_ms = now_ms;
    w.gesture_x += dx;
    w.gesture_y += dy;

    if interactive_dashboard(r) && (dx == 0.0 || r.pointer.wheel.fired) {
        if let Some((x, y)) = r.pointer.last
            && dy != 0.0
        {
            let kind = if dy > 0.0 {
                DashboardMouseKind::WheelUp
            } else {
                DashboardMouseKind::WheelDown
            };
            to_dashboard(r, kind, None, x, y);
        }
        return false;
    }

    // Sideways: once a gesture, when it is mostly sideways.
    let w = r.pointer.wheel;
    if !w.fired
        && w.gesture_x.abs() >= SIDEWAYS_WHEEL_MIN
        && w.gesture_x.abs() > 2.0 * w.gesture_y.abs()
    {
        r.pointer.wheel.fired = true;
        r.pointer.wheel.y = 0.0;
        // Two fingers moving right scroll left: back, as in a browser.
        return if w.gesture_x < 0.0 {
            go_back(r)
        } else {
            go_forward(r, None)
        };
    }
    if w.fired || editing(r) {
        return false;
    }

    match r.coordinate {
        Coordinate::Scroll | Coordinate::ScrollSearch => {
            let step = (r.hit_map.line_height.max(1.0) * 3.0 * dy) as i32;
            let max = (r.text_scroll_total_height - r.text_scroll_viewport_h).max(0);
            r.text_scroll_offset = (r.text_scroll_offset.max(0) - step).clamp(0, max);
            r.needs_redraw = true;
            false
        }
        Coordinate::Dashboard => false,
        _ => {
            r.pointer.wheel.y += dy;
            while r.pointer.wheel.y >= 1.0 && !quit {
                r.pointer.wheel.y -= 1.0;
                quit = press(r, Keycode::Up);
            }
            while r.pointer.wheel.y <= -1.0 && !quit {
                r.pointer.wheel.y += 1.0;
                quit = press(r, Keycode::Down);
            }
            quit
        }
    }
}

pub fn finger_down(r: &mut AppRenderer, id: u64, x: f32, y: f32, now_ms: u64) -> bool {
    if r.pointer.touch.is_some() {
        return false;
    }
    r.pointer.touch = Some(Touch {
        id,
        x0: x,
        y0: y,
        x,
        y,
        t0: now_ms,
        drag: TouchDrag::Undecided,
        rows: 0,
    });
    if interactive_dashboard(r) && r.hit_map.hit(x, y) == Some(Hit::Dashboard) {
        to_dashboard(r, DashboardMouseKind::Press, Some(Button::Left), x, y);
    }
    false
}

pub fn finger_motion(r: &mut AppRenderer, id: u64, x: f32, y: f32) -> bool {
    let Some(mut t) = r.pointer.touch.filter(|t| t.id == id) else {
        return false;
    };
    t.x = x;
    t.y = y;
    let (dx, dy) = (x - t.x0, y - t.y0);
    if t.drag == TouchDrag::Undecided && dx.hypot(dy) >= TAP_SLOP_PT * r.hit_map.px_per_pt {
        t.drag = if dy.abs() >= dx.abs() {
            TouchDrag::Vertical
        } else {
            TouchDrag::Sideways
        };
    }
    let mut quit = false;
    if interactive_dashboard(r) {
        r.pointer.touch = Some(t);
        to_dashboard(r, DashboardMouseKind::Motion, Some(Button::Left), x, y);
        return false;
    }
    if t.drag == TouchDrag::Vertical {
        // The list follows the finger: drawing it up brings the rows below.
        let want = (-dy / r.hit_map.line_height.max(1.0)) as i32;
        while t.rows < want && !quit {
            t.rows += 1;
            quit = press(r, Keycode::Down);
        }
        while t.rows > want && !quit {
            t.rows -= 1;
            quit = press(r, Keycode::Up);
        }
    }
    r.pointer.touch = Some(t);
    quit
}

pub fn finger_up(r: &mut AppRenderer, id: u64, x: f32, y: f32, now_ms: u64) -> bool {
    let Some(t) = r.pointer.touch.filter(|t| t.id == id) else {
        return false;
    };
    r.pointer.touch = None;
    if interactive_dashboard(r) && r.hit_map.hit(t.x0, t.y0) == Some(Hit::Dashboard) {
        to_dashboard(r, DashboardMouseKind::Release, Some(Button::Left), x, y);
        return false;
    }
    match t.drag {
        TouchDrag::Undecided => {
            let hit = r.hit_map.hit(t.x0, t.y0);
            let long = now_ms.saturating_sub(t.t0) >= LONG_PRESS_MS;
            let double = r
                .pointer
                .last_tap
                .is_some_and(|(at, tx, ty)| {
                    now_ms.saturating_sub(at) <= DOUBLE_TAP_MS
                        && (tx - t.x0).hypot(ty - t.y0) < TAP_SLOP_PT * r.hit_map.px_per_pt
                });
            r.pointer.last_tap = (!double).then_some((now_ms, t.x0, t.y0));
            if hit == Some(Hit::Field) && field_mode(r) {
                if double {
                    return press(r, Keycode::Return);
                }
                if let Some(at) = r.hit_map.caret_at(t.x0, t.y0) {
                    if long {
                        let (start, end) = handlers::word_around(r, at);
                        handlers::place_caret(r, end, Some(start));
                        handlers::announce_pointer_selection(r);
                    } else {
                        handlers::place_caret(r, at, None);
                    }
                }
                return false;
            }
            click(r, hit, t.x0, t.y0)
        }
        TouchDrag::Sideways => match swipe(&r.hit_map, x - t.x0, y - t.y0) {
            Some(true) => go_back(r),
            Some(false) => go_forward(r, r.hit_map.hit(t.x0, t.y0)),
            None => false,
        },
        TouchDrag::Vertical => false,
    }
}

/// Where the focus is drawn on the last frame, for a screen magnifier to follow
/// (AccessKit's bounds of the focused node): the caret while text is typed, the
/// selected target in element mode, and the selected row otherwise. Physical
/// pixels from the window's top left, as AccessKit wants them.
pub fn focus_rect(r: &AppRenderer) -> Option<Rect> {
    let map = &r.hit_map;
    if field_mode(r) {
        let at = r.cursor_position;
        if let Some((line, x)) = map
            .field
            .iter()
            .find_map(|l| l.stops.iter().find(|(b, _)| *b == at).map(|&(_, x)| (l, x)))
        {
            let w = 2.0 * map.px_per_pt.max(1.0);
            return Some(Rect::new(x, line.top, w, line.bottom - line.top));
        }
    }
    let row = r.list_index;
    let wanted = if r.coordinate == Coordinate::InElement {
        Hit::Target {
            row,
            target: r.nav_index,
        }
    } else {
        Hit::Row(row)
    };
    map.regions
        .iter()
        .find(|(_, hit)| *hit == wanted)
        .or_else(|| map.regions.iter().find(|(_, hit)| *hit == Hit::Row(row)))
        .map(|&(rect, _)| rect)
}

/// After a frame: put the caret where the click that opened the field was, now
/// that the field is laid out, and keep the cursor's shape true to what the frame
/// put under it.
pub fn after_frame(r: &mut AppRenderer) {
    if let Some((x, y)) = r.pointer.pending_caret.take()
        && editing(r)
        && let Some(at) = r.hit_map.caret_at(x, y)
    {
        handlers::place_caret(r, at, None);
    }
    if let Some((x, y)) = r.pointer.last {
        r.pointer.cursor = shape_at(r, x, y);
    }
}
