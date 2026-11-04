//! Where the rows of a list go on screen, without a GPU.
//!
//! This used to live inside `view::update_view` as separate passes: one measured
//! every row to decide what scrolls into view, another measured the visible rows
//! again to place them, and the draw worked some of it out a third time. They
//! disagreed in small ways, which is how a row's reserved height drifts from what
//! it draws. Now there is one measurement, [`measure_row`], and the scrolling, the
//! placing, the selection and the draw all read what it returns.
//!
//! Everything here is a plain function over a [`FontRenderer`] and a lookup for
//! image sizes, so it can be tested with the fake fonts in `text.rs`. `update_view`
//! cannot be: it needs a window and a Vulkan device.

use crate::text::FontRenderer;
use crate::view::{
    ext_flow, indicator_width, insert_buf_segments, label_for_display, prefix_for_display,
    split_image_label, split_label,
};

/// How one row looks. Every row has one: with no stylesheet it is the default sheet's.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct RowStyle {
    pub li: crate::style::Style,
    /// `li::marker`'s colour, for the list prefix.
    pub marker_color: Option<u32>,
    /// The cursor's row: the background it would have without the cursor, so the
    /// cursor can be told to differ from that.
    pub unfocused_background: Option<u32>,
    /// What `<b>` and `<span>` change inside the row's text.
    pub runs: Vec<TextRun>,
    /// The row is a tree of boxes (a grid, several pictures, a video), laid out by
    /// `boxes`, not a run of text.
    pub boxed: bool,
}

/// A stretch of a row's text, by character, that looks different from the rest.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TextRun {
    pub start: usize,
    pub end: usize,
    pub color: Option<u32>,
    pub bold: bool,
}

/// Cut `[from, to)` of a row's text at the runs: each piece, by character, with the
/// run it is in. In order, covering all of it.
pub(crate) fn split_at_runs(
    from: usize,
    to: usize,
    runs: &[TextRun],
) -> Vec<(usize, usize, Option<TextRun>)> {
    let mut out = Vec::new();
    let mut at = from;
    for run in runs {
        let (start, end) = (run.start.max(from), run.end.min(to));
        if start >= end {
            continue;
        }
        if at < start {
            out.push((at, start, None));
        }
        out.push((start, end, Some(*run)));
        at = end;
    }
    if at < to {
        out.push((at, to, None));
    }
    out
}

/// One row of the list being drawn, in the order it is drawn.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct ListItem {
    /// The label: list prefix, space, display text.
    pub label: String,
    /// An image path, or in extended search the breadcrumb.
    pub data: Option<String>,
    /// The cursor is on this row.
    pub is_selected: bool,
    /// Search match positions in `label`, by character.
    pub match_pos: Vec<u32>,
    /// In extended search, the path of the image this result shows.
    pub ext_prefix: Option<String>,
    pub style: std::sync::Arc<RowStyle>,
    /// The row's markup, for a row of boxes.
    pub markup: Option<String>,
}

/// A row's box in pixels: what its style adds around its text, on all four sides,
/// top right bottom left as CSS has it.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct RowBoxStyle {
    pub font_scale: f32,
    pub padding: [f32; 4],
    pub margin: [f32; 4],
    pub border: f32,
    pub radius: f32,
}

impl Default for RowBoxStyle {
    fn default() -> Self {
        RowBoxStyle {
            font_scale: 1.0,
            padding: [0.0; 4],
            margin: [0.0; 4],
            border: 0.0,
            radius: 0.0,
        }
    }
}

impl RowBoxStyle {
    /// From the row's style, with `column` the width of the list.
    pub fn resolve(
        style: &crate::style::Style,
        env: &crate::style::StyleEnv,
        column: f32,
    ) -> RowBoxStyle {
        let cx = crate::style::LengthContext {
            em: env.base_em * style.font_scale,
            ch: env.base_ch * style.font_scale,
            percent_of: column,
            px_scale: env.px_scale,
        };
        let sides = |sides: &crate::style::Sides| sides.map(|l| l.px(&cx).unwrap_or(0.0));
        RowBoxStyle {
            font_scale: style.font_scale,
            padding: sides(&style.padding),
            margin: sides(&style.margin),
            border: style.border_width.px(&cx).unwrap_or(0.0),
            radius: style.border_radius.px(&cx).unwrap_or(0.0),
        }
    }

    /// From the row's outer edge to its text, going down.
    pub fn top(&self) -> f32 {
        self.margin[0] + self.border + self.padding[0]
    }

    /// From the row's outer edge to its text, going right.
    pub fn left(&self) -> f32 {
        self.margin[3] + self.border + self.padding[3]
    }

    /// What the style adds to a row's height, outside its text.
    pub fn extra_height(&self) -> f32 {
        self.margin[0] + self.margin[2] + 2.0 * self.border + self.padding[0] + self.padding[2]
    }

    /// What the style takes from a row's width, outside its text.
    pub fn extra_width(&self) -> f32 {
        self.margin[1] + self.margin[3] + 2.0 * self.border + self.padding[1] + self.padding[3]
    }
}

/// The row being edited in insert mode, which measures its edit buffer instead of
/// its label.
pub(crate) struct EditedRow<'a> {
    pub index: usize,
    pub prefix: &'a str,
    pub buffer: &'a str,
}

/// What every row of one list is measured against.
pub(crate) struct ListMetrics<'a> {
    pub scale: f32,
    pub line_height: f32,
    pub em_width: f32,
    pub win_h: f32,
    /// Lines of chrome above the first row: the header and what follows it.
    pub header_lines: usize,
    pub max_content_w: f32,
    pub list_has_indicators: bool,
    /// Meta, timeline, a colon command's choices: the label is drawn in one piece.
    pub is_flat_list: bool,
    pub is_extended_search: bool,
    pub edited_row: Option<EditedRow<'a>>,
    /// What rows of boxes are laid out with.
    pub boxes: Option<BoxEnv<'a>>,
}

/// The stylesheets and units a row of boxes is laid out with.
pub(crate) struct BoxEnv<'a> {
    pub sheets: &'a crate::style::Sheets,
    pub env: &'a crate::style::StyleEnv,
}

/// How an image row divides its height, in lines, around the image.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct ImageLayout {
    pub prefix_lines: usize,
    pub suffix_lines: usize,
    pub image_lines: usize,
    pub img_w: f32,
    pub img_h: f32,
}

/// The size one row needs.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct RowShape {
    pub lines: usize,
    /// In pixels: its lines, at its font size, and what its style adds around them.
    pub height: f32,
    pub image: Option<ImageLayout>,
    pub style: RowBoxStyle,
    /// The boxes of a row of boxes, placed.
    pub boxes: Option<std::sync::Arc<crate::boxes::RowLayout>>,
}

/// One row, placed.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct RowBox {
    /// Baseline of the row's first line.
    pub y: f32,
    /// Top of the row's box, inside its margin.
    pub box_y: f32,
    pub content_x: f32,
    /// How far the row's text is pushed right by its style.
    pub left: f32,
    pub lines: usize,
    /// The row's box, inside its margin.
    pub box_h: f32,
    pub highlight_w: f32,
    pub image: Option<ImageLayout>,
    pub style: RowBoxStyle,
    pub boxes: Option<std::sync::Arc<crate::boxes::RowLayout>>,
}

/// Height of an image shown `w` wide, keeping its proportions. Without a usable
/// size it is square, the space a row keeps for an image that has not loaded.
pub(crate) fn scaled_image_height(size: Option<(u32, u32)>, w: f32) -> f32 {
    match size {
        Some((tw, th)) if tw != 0 => w * th as f32 / tw as f32,
        _ => w,
    }
}

/// Measure one row.
pub(crate) fn measure_row(
    fr: &FontRenderer,
    m: &ListMetrics<'_>,
    index: usize,
    item: &ListItem,
    style: &RowBoxStyle,
    image_size: &mut dyn FnMut(&str) -> Option<(u32, u32)>,
) -> RowShape {
    let editing_this = m.edited_row.as_ref().is_some_and(|e| e.index == index);
    if let (Some(be), Some(markup)) = (&m.boxes, &item.markup)
        && item.style.boxed
        && !editing_this
        && !m.is_extended_search
    {
        let width = (m.max_content_w - style.extra_width()).max(1.0);
        let laid = crate::boxes::layout_row(
            markup,
            item.is_selected,
            crate::boxes::RowInputs {
                sheets: be.sheets,
                env: be.env,
                fr,
                scale: m.scale,
                line_height: m.line_height,
            },
            width,
            image_size,
        );
        let lines = ((laid.height / m.line_height).ceil() as usize).max(1);
        return RowShape {
            lines,
            height: laid.height.max(m.line_height) + style.extra_height(),
            image: None,
            style: *style,
            boxes: Some(laid),
        };
    }
    let (lines, image) = measure_lines(fr, m, index, item, style, image_size);
    let body = match image {
        // An image's own height is not the font's.
        Some(_) => lines as f32 * m.line_height,
        None => lines as f32 * m.line_height * style.font_scale,
    };
    RowShape {
        lines,
        height: body + style.extra_height(),
        image,
        style: *style,
        boxes: None,
    }
}

fn measure_lines(
    fr: &FontRenderer,
    m: &ListMetrics<'_>,
    index: usize,
    item: &ListItem,
    style: &RowBoxStyle,
    image_size: &mut dyn FnMut(&str) -> Option<(u32, u32)>,
) -> (usize, Option<ImageLayout>) {
    let scale = m.scale * style.font_scale;
    let item_max_w = (m.max_content_w - style.extra_width()).max(1.0);
    let label = item.label.as_str();
    if let Some(edited) = m.edited_row.as_ref().filter(|e| e.index == index) {
        let pfx_w = fr.measure_text_width(edited.prefix, scale);
        let segments = insert_buf_segments(fr, edited.buffer, scale, pfx_w, item_max_w);
        return (segments.len().max(1), None);
    }
    if m.is_extended_search {
        return measure_search_result(fr, m, item, image_size);
    }
    if let Some(path) = &item.data {
        let (prefix, suffix, has_prefix) = split_image_label(label, path);
        // Captions wrap at the content column, exactly like an ordinary list item:
        // the "-p " tag sits on line 1 and only the text after it is measured.
        let prefix_lines = if has_prefix {
            let (_, prefix_content) = split_label(prefix);
            fr.count_wrapped_lines(prefix_content, scale, item_max_w)
        } else {
            0
        };
        let suffix_lines = if suffix.is_empty() {
            0
        } else {
            fr.count_wrapped_lines(suffix, scale, item_max_w)
        };
        let lh = m.line_height;
        let max_h_raw = m.win_h - lh * (m.header_lines + prefix_lines + suffix_lines) as f32;
        let max_h = if suffix_lines > 0 {
            ((max_h_raw / lh).floor() * lh).max(lh)
        } else {
            (max_h_raw - crate::text::TEXT_PADDING).max(lh)
        };
        let img_w = item_max_w;
        let img_h = scaled_image_height(image_size(path), img_w).min(max_h);
        let image_lines = ((img_h / lh).ceil() as usize).max(1);
        let image = ImageLayout {
            prefix_lines,
            suffix_lines,
            image_lines,
            img_w,
            img_h,
        };
        return (prefix_lines + image_lines + suffix_lines, Some(image));
    }
    if m.is_flat_list {
        // No prefix split: the whole label is drawn as text.
        return (
            fr.count_wrapped_lines(&label_for_display(label), scale, item_max_w),
            None,
        );
    }
    let (_, content) = split_label(label);
    (fr.count_wrapped_lines(content, scale, item_max_w), None)
}

/// An extended search result: the breadcrumb and prefix precede the content, which
/// narrows the room. Uses `4.0 * em_width`, the offset the draw uses, and subtracts
/// the indicator width whenever any row has one, since the text of all rows shifts
/// right by that much to stay aligned.
fn measure_search_result(
    fr: &FontRenderer,
    m: &ListMetrics<'_>,
    item: &ListItem,
    image_size: &mut dyn FnMut(&str) -> Option<(u32, u32)>,
) -> (usize, Option<ImageLayout>) {
    let scale = m.scale;
    let indicator_w = if m.list_has_indicators {
        indicator_width(m.line_height, m.em_width)
    } else {
        0.0
    };
    let rest_w = (m.max_content_w - 4.0 * m.em_width - indicator_w).max(1.0);
    let breadcrumb = item.data.as_deref().unwrap_or("");
    let label = item.label.as_str();
    if let Some(path) = &item.ext_prefix {
        // The breadcrumb and caption flow, then the image below.
        let img_w = rest_w;
        let img_h = scaled_image_height(image_size(path), img_w);
        let image_lines = ((img_h / m.line_height).ceil() as usize).max(1);
        let (prefix_text, suffix, _) = split_image_label(label, path);
        let caption = label_for_display(prefix_text);
        let prefix_lines =
            ext_flow(fr, breadcrumb, "", &caption, scale, img_w, m.em_width).total_lines;
        let suffix_lines = if suffix.is_empty() {
            0
        } else {
            fr.count_wrapped_lines(suffix, scale, img_w)
        };
        let image = ImageLayout {
            prefix_lines,
            suffix_lines,
            image_lines,
            img_w,
            img_h,
        };
        return (prefix_lines + image_lines + suffix_lines, Some(image));
    }
    let (prefix, content) = split_label(label);
    let shown_prefix = prefix_for_display(prefix);
    let lines = ext_flow(
        fr,
        breadcrumb,
        &shown_prefix,
        content,
        scale,
        rest_w,
        m.em_width,
    );
    (lines.total_lines, None)
}

/// The first row to draw, so that the selected row is on screen.
///
/// `scroll_offset` is the row the view last started at, or below zero to put the
/// selected row last on screen, which is what paging down asks for.
pub(crate) fn first_visible_row(
    heights: &[f32],
    viewport_h: f32,
    selected: usize,
    scroll_offset: i32,
) -> usize {
    if heights.is_empty() {
        return 0;
    }
    let height = |i: usize| heights.get(i).copied().unwrap_or(0.0);
    // When every row fits, always start at the top: a non-zero offset (set by
    // walking back to the selected row, say) would only hide rows above the cursor.
    if heights.iter().sum::<f32>() <= viewport_h {
        return 0;
    }
    if scroll_offset < 0 {
        let mut used = height(selected);
        let mut first = selected;
        while first > 0 {
            let above = height(first - 1);
            if used + above > viewport_h {
                break;
            }
            used += above;
            first -= 1;
        }
        return first;
    }
    let mut first = (scroll_offset as usize).min(selected);
    // Snap forward until the selected row is within the visible area.
    let mut used: f32 = heights[first..=selected.min(heights.len() - 1)]
        .iter()
        .sum();
    while used > viewport_h && first < selected {
        used -= height(first);
        first += 1;
    }
    // Scrolloff: try to show one row above the selection.
    if first > 0 && first == selected && used + height(first - 1) <= viewport_h {
        first -= 1;
    }
    first
}

/// Place rows from `start`, stopping past the bottom of the window. `ascent` is how
/// far a row's top is above its first baseline at the app's font size.
pub(crate) fn place_rows(
    shapes: &[RowShape],
    start: usize,
    first_y: f32,
    win_h: f32,
    content_x: f32,
    highlight_w: f32,
    ascent: f32,
) -> Vec<RowBox> {
    let mut y = first_y;
    let mut rows = Vec::with_capacity(shapes.len().saturating_sub(start));
    for shape in shapes.iter().skip(start) {
        if y > win_h {
            break;
        }
        let s = &shape.style;
        rows.push(RowBox {
            // A bigger font's first baseline sits lower.
            y: y + s.top() + ascent * (s.font_scale - 1.0),
            box_y: y - ascent + s.margin[0],
            content_x: content_x + s.left(),
            left: s.left(),
            lines: shape.lines,
            box_h: shape.height - s.margin[0] - s.margin[2],
            highlight_w: highlight_w - s.margin[1] - s.margin[3],
            image: shape.image,
            style: *s,
            boxes: shape.boxes.clone(),
        });
        y += shape.height;
    }
    rows
}

/// Each row's height in whole lines, rounded up, for the handlers' paging, which
/// budgets a page in lines.
pub(crate) fn line_counts(shapes: &[RowShape], line_height: f32) -> Vec<usize> {
    shapes
        .iter()
        .map(|s| (s.height / line_height.max(1.0)).ceil() as usize)
        .collect()
}

/// Where a target of the focused row starts on screen, in pixels from the row.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct TargetPos {
    pub x: f32,
    pub y: f32,
}

/// Where each target starts in `content` as it wraps at `wrap_w`. Spans are by
/// character in `content`.
pub(crate) fn target_positions(
    fr: &FontRenderer,
    spans: impl Iterator<Item = usize>,
    content: &str,
    scale: f32,
    wrap_w: f32,
    line_height: f32,
) -> Vec<TargetPos> {
    let lines = fr.line_segments(content, scale, Some((wrap_w, wrap_w)));
    spans
        .map(|start| {
            let line = lines
                .iter()
                .rposition(|&(_, _, first_char)| first_char as usize <= start)
                .unwrap_or(0);
            let x = lines.get(line).map_or(0.0, |&(from, to, first_char)| {
                let into_line = start.saturating_sub(first_char as usize);
                let upto = content[from..to]
                    .char_indices()
                    .nth(into_line)
                    .map_or(to, |(at, _)| from + at);
                fr.measure_text_width(&content[from..upto], scale)
            });
            TargetPos {
                x,
                y: line as f32 * line_height,
            }
        })
        .collect()
}

/// The target on the nearest line above or below `from`, and of those the nearest
/// sideways, the first on a tie. `None` with no line in that direction.
pub(crate) fn target_on_next_line(
    positions: &[TargetPos],
    from: usize,
    down: bool,
) -> Option<usize> {
    let here = positions.get(from)?;
    let beyond = |p: &TargetPos| if down { p.y > here.y } else { p.y < here.y };
    let line = positions
        .iter()
        .filter(|p| beyond(p))
        .map(|p| p.y)
        .reduce(|a, b| if down { a.min(b) } else { a.max(b) })?;
    let mut best: Option<(usize, f32)> = None;
    for (i, p) in positions.iter().enumerate() {
        if p.y == line {
            let off = (p.x - here.x).abs();
            if best.is_none_or(|(_, b)| off < b) {
                best = Some((i, off));
            }
        }
    }
    best.map(|(i, _)| i)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::text::make_fr_uniform;

    const LH: f32 = 20.0;
    const EM: f32 = 10.0;

    /// 10 px a character, a 200 px column, a 400 px window with two lines of chrome.
    fn metrics() -> ListMetrics<'static> {
        ListMetrics {
            scale: 1.0,
            line_height: LH,
            em_width: EM,
            win_h: 400.0,
            header_lines: 2,
            max_content_w: 200.0,
            list_has_indicators: false,
            is_flat_list: false,
            is_extended_search: false,
            edited_row: None,
            boxes: None,
        }
    }

    fn row(label: &str) -> ListItem {
        ListItem {
            label: label.into(),
            ..Default::default()
        }
    }

    fn image_row(label: &str, path: &str) -> ListItem {
        ListItem {
            label: label.into(),
            data: Some(path.into()),
            ..Default::default()
        }
    }

    fn measure(m: &ListMetrics<'_>, item: &ListItem, size: Option<(u32, u32)>) -> RowShape {
        measure_row(
            &make_fr_uniform(EM),
            m,
            0,
            item,
            &RowBoxStyle::default(),
            &mut |_| size,
        )
    }

    fn shape(lines: usize) -> RowShape {
        RowShape {
            lines,
            height: lines as f32 * LH,
            image: None,
            style: RowBoxStyle::default(),
            boxes: None,
        }
    }

    fn heights(lines: &[usize]) -> Vec<f32> {
        lines.iter().map(|l| *l as f32 * LH).collect()
    }

    fn pos(x: f32, y: f32) -> TargetPos {
        TargetPos { x, y }
    }

    fn styled() -> RowBoxStyle {
        RowBoxStyle {
            font_scale: 1.0,
            padding: [1.0, 2.0, 3.0, 4.0],
            margin: [10.0, 20.0, 30.0, 40.0],
            border: 5.0,
            radius: 0.0,
        }
    }

    #[test]
    fn a_style_adds_around_the_row_and_narrows_its_text() {
        let fr = make_fr_uniform(EM);
        let m = metrics();
        let shape = measure_row(&fr, &m, 0, &row("- 12345678901234"), &styled(), &mut |_| {
            None
        });
        // 14 characters, 140 px, in 200 - (20 + 40 + 10 + 2 + 4) = 124 px: two lines.
        assert_eq!(shape.lines, 2);
        assert_eq!(shape.height, 2.0 * LH + 10.0 + 30.0 + 10.0 + 1.0 + 3.0);
    }

    #[test]
    fn a_bigger_font_takes_more_room() {
        let fr = make_fr_uniform(EM);
        let m = metrics();
        let big = RowBoxStyle {
            font_scale: 2.0,
            ..Default::default()
        };
        // 11 characters at 20 px each is 220 px in 200.
        let shape = measure_row(&fr, &m, 0, &row("- hello there"), &big, &mut |_| None);
        assert_eq!(shape.lines, 2);
        assert_eq!(shape.height, 2.0 * 2.0 * LH);
    }

    #[test]
    fn a_placed_row_knows_its_box_and_where_its_text_starts() {
        let shape = RowShape {
            lines: 1,
            height: LH + styled().extra_height(),
            image: None,
            style: styled(),
            boxes: None,
        };
        let rows = place_rows(
            &[shape.clone(), shape.clone()],
            0,
            100.0,
            1000.0,
            7.0,
            500.0,
            16.0,
        );
        assert_eq!(rows[0].y, 100.0 + 10.0 + 5.0 + 1.0);
        assert_eq!(rows[0].box_y, 100.0 - 16.0 + 10.0);
        assert_eq!(rows[0].box_h, LH + 5.0 + 5.0 + 1.0 + 3.0);
        assert_eq!(rows[0].content_x, 7.0 + 40.0 + 5.0 + 4.0);
        assert_eq!(rows[0].highlight_w, 500.0 - 20.0 - 40.0);
        assert_eq!(rows[1].y, rows[0].y + shape.height);
    }

    /// The row every list had before stylesheets, unchanged to the pixel.
    #[test]
    fn no_style_adds_nothing() {
        let row = place_rows(&[shape(3)], 0, 100.0, 1000.0, 7.0, 500.0, 16.0).remove(0);
        assert_eq!((row.y, row.content_x, row.left), (100.0, 7.0, 0.0));
        assert_eq!(
            (row.box_y, row.box_h, row.highlight_w),
            (84.0, 3.0 * LH, 500.0)
        );
    }

    #[test]
    fn a_style_resolves_against_the_font_and_the_column() {
        use crate::style::{Length, Style, StyleEnv};
        let env = StyleEnv {
            base_em: 20.0,
            base_ch: 10.0,
            px_scale: 2.0,
            ..Default::default()
        };
        let style = Style {
            font_scale: 2.0,
            padding: [
                Length::Em(1.0),
                Length::Ch(1.0),
                Length::Px(1.0),
                Length::Percent(10.0),
            ],
            border_width: Length::Px(3.0),
            ..Default::default()
        };
        let got = RowBoxStyle::resolve(&style, &env, 300.0);
        assert_eq!(got.padding, [40.0, 20.0, 2.0, 30.0]);
        assert_eq!(got.border, 6.0);
    }

    #[test]
    fn targets_are_placed_where_the_text_wraps_them() {
        let fr = make_fr_uniform(EM);
        // "aaaaaaaaa bbbbbbbbb " fills the 200 px line, so "ccc" starts the next.
        let content = "aaaaaaaaa bbbbbbbbb ccc";
        let at = target_positions(&fr, [0, 10, 20].into_iter(), content, 1.0, 200.0, LH);
        assert_eq!(at[0], TargetPos { x: 0.0, y: 0.0 });
        assert_eq!(at[1], TargetPos { x: 100.0, y: 0.0 });
        assert_eq!(at[2], TargetPos { x: 0.0, y: LH });
    }

    /// A grid of three by two: the arrows move by picture row, in the same column.
    #[test]
    fn up_and_down_keep_the_column_in_a_grid() {
        let grid = [
            pos(0.0, 0.0),
            pos(100.0, 0.0),
            pos(200.0, 0.0),
            pos(0.0, 50.0),
            pos(100.0, 50.0),
            pos(200.0, 50.0),
        ];
        assert_eq!(target_on_next_line(&grid, 1, true), Some(4));
        assert_eq!(target_on_next_line(&grid, 5, false), Some(2));
        assert_eq!(target_on_next_line(&grid, 4, true), None);
        assert_eq!(target_on_next_line(&grid, 1, false), None);
    }

    /// Only the nearest line counts, even when a further one is straight below.
    #[test]
    fn a_line_is_not_skipped_for_a_better_column() {
        let at = [pos(100.0, 0.0), pos(0.0, 20.0), pos(100.0, 40.0)];
        assert_eq!(target_on_next_line(&at, 0, true), Some(1));
    }

    #[test]
    fn a_position_that_does_not_exist_goes_nowhere() {
        assert_eq!(target_on_next_line(&[], 0, true), None);
    }

    fn run(start: usize, end: usize) -> TextRun {
        TextRun {
            start,
            end,
            color: None,
            bold: true,
        }
    }

    #[test]
    fn a_line_with_no_runs_is_one_piece() {
        assert_eq!(split_at_runs(0, 5, &[]), vec![(0, 5, None)]);
    }

    #[test]
    fn a_line_is_cut_around_its_runs() {
        let r = run(2, 4);
        assert_eq!(
            split_at_runs(0, 6, &[r]),
            vec![(0, 2, None), (2, 4, Some(r)), (4, 6, None)]
        );
    }

    /// A run wrapped over two lines is cut at the line's edges.
    #[test]
    fn a_run_is_clipped_to_the_line() {
        let r = run(3, 10);
        assert_eq!(
            split_at_runs(0, 5, &[r]),
            vec![(0, 3, None), (3, 5, Some(r))]
        );
        assert_eq!(split_at_runs(5, 8, &[r]), vec![(5, 8, Some(r))]);
        assert_eq!(
            split_at_runs(8, 12, &[r]),
            vec![(8, 10, Some(r)), (10, 12, None)]
        );
        assert_eq!(split_at_runs(11, 12, &[r]), vec![(11, 12, None)]);
    }

    #[test]
    fn a_short_row_is_one_line() {
        let s = measure(&metrics(), &row("- short"), None);
        assert_eq!((s.lines, s.height), (1, LH));
        assert_eq!(s.image, None);
    }

    /// The prefix does not count: it has its own column.
    #[test]
    fn a_long_row_wraps_on_its_content_alone() {
        // 30 characters of content at 10 px in a 200 px column.
        let s = measure(&metrics(), &row("- aaaaaaaaa bbbbbbbbb ccccccccc"), None);
        assert_eq!(s.lines, 2);
        assert_eq!(s.height, 2.0 * LH);
    }

    /// A flat list draws its label whole, prefix included, so it has to measure
    /// that too. Measuring only the part after the first space made paging and
    /// the draw disagree about these rows.
    #[test]
    fn a_flat_row_wraps_on_its_whole_label() {
        let mut m = metrics();
        m.is_flat_list = true;
        // 19 characters after the space fit in 200 px, but not with what precedes.
        let label = "aaaaaaaaaaaa bbbbbbbbbbbbbbbbbbb";
        assert_eq!(measure(&metrics(), &row(label), None).lines, 1);
        assert_eq!(measure(&m, &row(label), None).lines, 2);
    }

    #[test]
    fn the_edited_row_measures_its_buffer_not_its_label() {
        let mut m = metrics();
        m.edited_row = Some(EditedRow {
            index: 0,
            prefix: "",
            buffer: "one\ntwo\nthree",
        });
        assert_eq!(measure(&m, &row("- label"), None).lines, 3);
    }

    #[test]
    fn only_the_edited_row_measures_a_buffer() {
        let mut m = metrics();
        m.edited_row = Some(EditedRow {
            index: 5,
            prefix: "",
            buffer: "one\ntwo\nthree",
        });
        assert_eq!(measure(&m, &row("- label"), None).lines, 1);
    }

    /// 200 px wide and 100 px high is five lines, keeping its proportions.
    #[test]
    fn an_image_row_keeps_the_proportions_of_its_image() {
        let s = measure(
            &metrics(),
            &image_row("-p a.jpg", "a.jpg"),
            Some((400, 200)),
        );
        let image = s.image.expect("image layout");
        assert_eq!((image.img_w, image.img_h), (200.0, 100.0));
        assert_eq!(image.image_lines, 5);
        assert_eq!(s.lines, 5);
    }

    /// The caption counts its own lines, and the bare `-p` tag none.
    #[test]
    fn an_image_row_counts_its_captions() {
        let item = image_row("-p before a.jpg after", "a.jpg");
        let image = measure(&metrics(), &item, Some((400, 200)))
            .image
            .expect("image");
        assert_eq!((image.prefix_lines, image.suffix_lines), (1, 1));
        assert_eq!(measure(&metrics(), &item, Some((400, 200))).lines, 7);
    }

    /// The window is 400 px high, so a very tall image cannot take more than what
    /// the chrome leaves.
    #[test]
    fn a_tall_image_is_capped_to_the_window() {
        let s = measure(
            &metrics(),
            &image_row("-p a.jpg", "a.jpg"),
            Some((100, 1000)),
        );
        let image = s.image.expect("image");
        assert_eq!(image.img_h, 400.0 - 2.0 * LH - crate::text::TEXT_PADDING);
    }

    /// The same cap whether or not the image has loaded, or the row would jump
    /// when it does. This is where the old passes disagreed.
    #[test]
    fn an_image_that_has_not_loaded_is_capped_the_same_way() {
        let mut m = metrics();
        m.win_h = 100.0;
        let s = measure(&m, &image_row("-p a.jpg", "a.jpg"), None);
        let image = s.image.expect("image");
        assert_eq!(image.img_h, 100.0 - 2.0 * LH - crate::text::TEXT_PADDING);
    }

    #[test]
    fn a_zero_width_image_counts_as_square() {
        assert_eq!(scaled_image_height(Some((0, 50)), 80.0), 80.0);
        assert_eq!(scaled_image_height(None, 80.0), 80.0);
        assert_eq!(scaled_image_height(Some((40, 20)), 80.0), 40.0);
    }

    #[test]
    fn a_search_result_uses_the_same_flow_as_its_draw() {
        let mut m = metrics();
        m.is_extended_search = true;
        let result = |crumb: &str| ListItem {
            label: "- content".into(),
            data: Some(crumb.into()),
            ..Default::default()
        };
        // The column is 160 wide. A short breadcrumb leaves room to go on inline, a
        // longer one leaves too little, and the content drops to a line of its own.
        assert_eq!(measure(&m, &result("a > "), None).lines, 1);
        assert_eq!(measure(&m, &result("crumb > "), None).lines, 2);
    }

    /// In extended search `data` is a breadcrumb, never an image, so only
    /// `ext_prefix` makes an image row.
    #[test]
    fn a_search_result_shows_the_image_of_its_ext_prefix() {
        let mut m = metrics();
        m.is_extended_search = true;
        let item = ListItem {
            label: "-p a.jpg".into(),
            data: Some("crumb > ".into()),
            ext_prefix: Some("a.jpg".into()),
            ..Default::default()
        };
        // 200 - 4 em = 160 wide, 80 high: four lines, plus the breadcrumb's.
        let s = measure(&m, &item, Some((400, 200)));
        let image = s.image.expect("image");
        assert_eq!(
            (image.img_w, image.img_h, image.image_lines),
            (160.0, 80.0, 4)
        );
        assert_eq!(s.lines, image.prefix_lines + 4);
    }

    /// Indicators shift every row's text, which narrows a search result too.
    #[test]
    fn indicators_narrow_a_search_result() {
        let mut plain = metrics();
        plain.is_extended_search = true;
        let mut with = metrics();
        with.is_extended_search = true;
        with.list_has_indicators = true;
        let item = ListItem {
            label: "-p a.jpg".into(),
            ext_prefix: Some("a.jpg".into()),
            ..Default::default()
        };
        let w = |m: &ListMetrics<'_>| measure(m, &item, None).image.expect("image").img_w;
        assert_eq!(w(&plain) - w(&with), indicator_width(LH, EM));
    }

    #[test]
    fn a_list_that_fits_starts_at_the_top() {
        let h = heights(&[1, 1, 1]);
        assert_eq!(first_visible_row(&h, 3.0 * LH, 2, 2), 0);
        assert_eq!(first_visible_row(&h, 3.0 * LH, 2, -1), 0);
    }

    #[test]
    fn an_empty_list_starts_at_the_top() {
        assert_eq!(first_visible_row(&[], 100.0, 0, 3), 0);
    }

    #[test]
    fn paging_down_puts_the_selection_last_on_screen() {
        let h = heights(&[1, 1, 1, 1, 1, 1]);
        assert_eq!(first_visible_row(&h, 3.0 * LH, 4, -1), 2);
    }

    /// A tall row above stops the walk rather than being half shown.
    #[test]
    fn paging_down_stops_at_a_row_that_does_not_fit() {
        let h = heights(&[1, 5, 1, 1, 1]);
        assert_eq!(first_visible_row(&h, 3.0 * LH, 4, -1), 2);
    }

    #[test]
    fn the_view_keeps_its_place_while_the_selection_stays_on_screen() {
        let h = heights(&[1, 1, 1, 1, 1, 1]);
        assert_eq!(first_visible_row(&h, 3.0 * LH, 3, 2), 2);
    }

    #[test]
    fn the_view_follows_a_selection_that_moved_below_it() {
        let h = heights(&[1, 1, 1, 1, 1, 1]);
        assert_eq!(first_visible_row(&h, 3.0 * LH, 5, 0), 3);
    }

    #[test]
    fn the_view_shows_one_row_above_a_selection_that_moved_above_it() {
        let h = heights(&[1, 1, 1, 1, 1, 1]);
        assert_eq!(first_visible_row(&h, 3.0 * LH, 2, 4), 1);
    }

    /// Taller than the screen: it still has to be the row that is shown.
    #[test]
    fn a_selection_taller_than_the_screen_is_still_shown() {
        let h = heights(&[1, 10, 1]);
        assert_eq!(first_visible_row(&h, 3.0 * LH, 1, 0), 1);
    }

    #[test]
    fn rows_are_placed_one_under_the_other_from_the_start() {
        let shapes = [shape(1), shape(2), shape(3)];
        let rows = place_rows(&shapes, 1, 50.0, 400.0, 7.0, 9.0, 16.0);
        assert_eq!(rows.len(), 2);
        assert_eq!((rows[0].y, rows[0].lines), (50.0, 2));
        assert_eq!((rows[1].y, rows[1].lines), (50.0 + 2.0 * LH, 3));
        assert_eq!((rows[1].content_x, rows[1].highlight_w), (7.0, 9.0));
    }

    /// One row may start below the fold's last line, so nothing is cut off early.
    #[test]
    fn placing_stops_past_the_bottom_of_the_window() {
        let shapes = vec![shape(1); 10];
        let rows = place_rows(&shapes, 0, 0.0, 2.0 * LH, 0.0, 0.0, 16.0);
        assert_eq!(rows.len(), 3, "rows at 0, one line and exactly the edge");
    }

    #[test]
    fn paging_counts_a_part_of_a_line_as_a_line() {
        let mut tall = shape(1);
        tall.height = 1.5 * LH;
        assert_eq!(line_counts(&[shape(2), tall], LH), vec![2, 2]);
    }
}
