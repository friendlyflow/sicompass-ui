//! Rows that hold boxes: a grid of photos, a card with a heading and a picture, a
//! flex row of thumbnails.
//!
//! An ordinary row is a run of text, and everything about it is lines. A row of markup
//! with `display: grid` or `flex` in it, or with several images or a video, is a tree of
//! boxes instead. [`build`] reads the row into that tree, with each element's style from
//! the cascade, and [`layout`] gives every box a place through `taffy`, the CSS grid and
//! flexbox engine. [`RowLayout`] is the result: boxes to paint, in paint order, and where
//! each of the row's targets (images, videos, links, buttons) sits, which is what
//! navigate mode moves between.
//!
//! The tree is made of three things. Block elements (`div`, `p`, `h1`...) make
//! containers. Runs of text, with the inline elements in them (`b`, `span`, `a`), make
//! text blocks that wrap. And each `<img>` and `<video>` makes a box of its own, a block
//! whatever its tag sits inside.
//!
//! Nothing here touches the GPU, so all of it is tested with the fake fonts of `text.rs`.

use crate::layout::TextRun;
use crate::style::{
    Display, Justify, Length, LengthContext, ObjectFit, Sheets, Style, StyleEnv, StyleNode,
    TextAlign,
};
use crate::text::FontRenderer;
use sicompass_sdk::markup::{self, Element, Node};
use sicompass_sdk::tags;
use taffy::style_helpers::{auto, fr, length, percent};
use taffy::{
    AvailableSpace, Dimension, GridPlacement, GridTemplateComponent, JustifyContent,
    LengthPercentage, LengthPercentageAuto, Line, NodeId, Rect, Size, TaffyTree,
};

/// What a box holds.
#[derive(Debug, Clone, PartialEq)]
pub(crate) enum Content {
    Container,
    Text {
        text: String,
        runs: Vec<TextRun>,
    },
    Media {
        path: String,
        is_video: bool,
        name: String,
        poster: Option<String>,
    },
}

/// One box of the tree, before it has a place.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct BoxNode {
    pub style: Style,
    pub content: Content,
    pub children: Vec<BoxNode>,
    /// How many of the row's targets are in this box.
    pub targets: usize,
}

const BLOCKS: &[&str] = &["div", "p", "h1", "h2", "h3", "header"];

fn is_media(element: &Element) -> bool {
    matches!(element.tag, "img" | "video")
}

fn has_media(nodes: &[Node]) -> bool {
    nodes.iter().any(|node| match node {
        Node::Element(element) => is_media(element) || has_media(&element.children),
        Node::Text(_) => false,
    })
}

fn node_of(element: &Element) -> StyleNode {
    StyleNode {
        tag: element.tag.to_owned(),
        classes: element.classes.clone(),
        focus: false,
    }
}

/// Inline content waiting to become a text block.
#[derive(Default)]
struct Flow {
    nodes: Vec<Node>,
    /// Targets that belong to the box before or after, not to any text of this flow.
    carry: usize,
}

struct Builder<'a> {
    sheets: &'a Sheets,
    env: &'a StyleEnv,
    path: Vec<StyleNode>,
}

impl Builder<'_> {
    fn style(&self) -> Style {
        self.sheets.style_for(&self.path, self.env)
    }

    fn container(&mut self, children: &[Node], style: Style) -> BoxNode {
        let mut out = Vec::new();
        let mut flow = Flow::default();
        self.walk(children, &style, &mut out, &mut flow);
        self.flush(&style, &mut flow, &mut out);
        BoxNode {
            style,
            content: Content::Container,
            children: out,
            targets: 0,
        }
    }

    fn add_targets(out: &mut [BoxNode], flow: &mut Flow, count: usize) {
        match out.last_mut() {
            Some(last) => last.targets += count,
            None => flow.carry += count,
        }
    }

    fn walk(&mut self, nodes: &[Node], parent: &Style, out: &mut Vec<BoxNode>, flow: &mut Flow) {
        for node in nodes {
            let Node::Element(element) = node else {
                flow.nodes.push(node.clone());
                continue;
            };
            if is_media(element) {
                self.flush(parent, flow, out);
                out.push(self.media(element));
            } else if BLOCKS.contains(&element.tag) {
                self.flush(parent, flow, out);
                self.path.push(node_of(element));
                let style = self.style();
                let child = self.container(&element.children, style);
                self.path.pop();
                out.push(child);
            } else if has_media(&element.children) {
                // An inline element around a picture: the picture is still a box.
                self.path.push(node_of(element));
                self.walk(&element.children, parent, out, flow);
                self.path.pop();
                if element.tag == "a" && element.href.is_some() {
                    self.flush(parent, flow, out);
                    Self::add_targets(out, flow, 1);
                }
            } else {
                flow.nodes.push(node.clone());
            }
        }
    }

    fn flush(&mut self, parent: &Style, flow: &mut Flow, out: &mut Vec<BoxNode>) {
        let nodes = std::mem::take(&mut flow.nodes);
        if nodes.is_empty() {
            if flow.carry > 0 && !out.is_empty() {
                let carry = std::mem::take(&mut flow.carry);
                Self::add_targets(out, flow, carry);
            }
            return;
        }
        let shown = tags::strip_display(&markup::plain_nodes(&nodes)).replace('\n', " ");
        let targets = markup::targets_of(&nodes).len() + std::mem::take(&mut flow.carry);
        let text = shown.trim();
        if text.is_empty() {
            if targets > 0 {
                Self::add_targets(out, flow, targets);
            }
            return;
        }
        let lead = shown.chars().take_while(|c| c.is_whitespace()).count();
        let len = text.chars().count();
        let runs = self
            .sheets
            .runs_for(&self.path, &nodes, self.env)
            .into_iter()
            .filter_map(|run| {
                let start = run.start.saturating_sub(lead);
                let end = run.end.saturating_sub(lead).min(len);
                (start < end).then_some(TextRun { start, end, ..run })
            })
            .collect();
        out.push(BoxNode {
            style: parent.inherited(),
            content: Content::Text {
                text: text.to_owned(),
                runs,
            },
            children: Vec::new(),
            targets,
        });
    }

    fn media(&mut self, element: &Element) -> BoxNode {
        self.path.push(node_of(element));
        let style = self.style();
        self.path.pop();
        let found = markup::targets_of(std::slice::from_ref(&Node::Element(element.clone())));
        let (path, name) = found
            .into_iter()
            .next()
            .map(|t| (t.value, t.name))
            .unwrap_or_default();
        BoxNode {
            style,
            content: Content::Media {
                path,
                is_video: element.tag == "video",
                name,
                poster: element.poster.clone(),
            },
            children: Vec::new(),
            targets: 1,
        }
    }
}

/// Read a row of markup into boxes. `row_path` is the row's place in the list, from
/// [`Sheets::row_path`].
pub(crate) fn build(
    row: &str,
    sheets: &Sheets,
    env: &StyleEnv,
    row_path: Vec<StyleNode>,
) -> BoxNode {
    let nodes = markup::parse(row);
    let mut builder = Builder {
        sheets,
        env,
        path: row_path,
    };
    match markup::whole_row_element(&nodes) {
        Some(element) if !is_media(element) && BLOCKS.contains(&element.tag) => {
            builder.path.push(node_of(element));
            let style = builder.style();
            builder.container(&element.children, style)
        }
        _ => {
            let style = builder.style();
            builder.container(&nodes, style)
        }
    }
}

// ---------------------------------------------------------------------------
// Layout
// ---------------------------------------------------------------------------

/// A box with a place, relative to the row's top left.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct Placed {
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
    pub style: Style,
    /// Border and padding in pixels, top right bottom left, which is where text starts.
    pub inset: [f32; 4],
    /// The border's width in pixels.
    pub border: f32,
    pub radius: f32,
    /// What is behind this box: its own background, else that of the box around it.
    pub backdrop: Option<u32>,
    pub content: PlacedContent,
}

#[derive(Debug, Clone, PartialEq)]
pub(crate) enum PlacedContent {
    Container,
    Text {
        /// Each wrapped line, with the character it starts at.
        lines: Vec<(String, usize)>,
        runs: Vec<TextRun>,
        /// The scale to draw at, from the app's.
        scale: f32,
        line_height: f32,
    },
    Media {
        path: String,
        is_video: bool,
        name: String,
        poster: Option<String>,
        fit: ObjectFit,
    },
}

/// Where a target is, in pixels from the row's top left.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct TargetRect {
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
}

/// A row of boxes, placed.
#[derive(Debug, Clone, PartialEq, Default)]
pub(crate) struct RowLayout {
    pub width: f32,
    pub height: f32,
    /// In paint order: a box comes before the boxes inside it.
    pub boxes: Vec<Placed>,
    /// By target index, as `markup::targets` numbers them.
    pub targets: Vec<TargetRect>,
}

/// What laying out a row needs to know.
pub(crate) struct LayoutCtx<'a> {
    pub fr: &'a FontRenderer,
    /// The text scale at the app's font size.
    pub scale: f32,
    pub line_height: f32,
    pub env: &'a StyleEnv,
}

enum Leaf {
    Text {
        text: String,
        scale: f32,
        line_height: f32,
    },
    Media {
        /// Width over height.
        aspect: f32,
        /// Its own width in pixels, or what to assume.
        natural_w: f32,
    },
}

fn to_dimension(len: Length, cx: &LengthContext) -> Dimension {
    match len {
        Length::Percent(p) => percent(p / 100.0),
        Length::Auto | Length::Fr(_) => auto(),
        other => length(other.px(cx).unwrap_or(0.0)),
    }
}

fn to_lp(len: Length, cx: &LengthContext) -> LengthPercentage {
    match len {
        Length::Percent(p) => percent(p / 100.0),
        other => length(other.px(cx).unwrap_or(0.0)),
    }
}

fn to_lpa(len: Length, cx: &LengthContext) -> LengthPercentageAuto {
    match len {
        Length::Auto => auto(),
        Length::Percent(p) => percent(p / 100.0),
        other => length(other.px(cx).unwrap_or(0.0)),
    }
}

fn to_tracks(tracks: &[Length], cx: &LengthContext) -> Vec<GridTemplateComponent<String>> {
    tracks
        .iter()
        .map(|track| match *track {
            Length::Fr(v) => fr(v),
            Length::Auto => auto(),
            Length::Percent(p) => percent(p / 100.0),
            other => length(other.px(cx).unwrap_or(0.0)),
        })
        .collect()
}

fn length_context(style: &Style, env: &StyleEnv, column: f32) -> LengthContext {
    LengthContext {
        em: env.base_em * style.font_scale,
        ch: env.base_ch * style.font_scale,
        percent_of: column,
        px_scale: env.px_scale,
    }
}

fn taffy_style(style: &Style, env: &StyleEnv, column: f32) -> taffy::Style {
    let cx = length_context(style, env, column);
    let sides = |s: &[Length; 4]| Rect {
        top: to_lp(s[0], &cx),
        right: to_lp(s[1], &cx),
        bottom: to_lp(s[2], &cx),
        left: to_lp(s[3], &cx),
    };
    let border: LengthPercentage = to_lp(style.border_width, &cx);
    taffy::Style {
        display: match style.display {
            Display::Block => taffy::Display::Block,
            Display::Grid => taffy::Display::Grid,
            Display::Flex => taffy::Display::Flex,
        },
        size: Size {
            width: to_dimension(style.width, &cx),
            height: to_dimension(style.height, &cx),
        },
        max_size: Size {
            width: style.max_width.map_or_else(auto, |w| to_dimension(w, &cx)),
            height: auto(),
        },
        margin: Rect {
            top: to_lpa(style.margin[0], &cx),
            right: to_lpa(style.margin[1], &cx),
            bottom: to_lpa(style.margin[2], &cx),
            left: to_lpa(style.margin[3], &cx),
        },
        padding: sides(&style.padding),
        border: Rect {
            top: border,
            right: border,
            bottom: border,
            left: border,
        },
        gap: Size {
            width: to_lp(style.gap, &cx),
            height: to_lp(style.gap, &cx),
        },
        grid_template_columns: to_tracks(&style.grid_columns, &cx),
        grid_template_rows: to_tracks(&style.grid_rows, &cx),
        grid_column: Line {
            start: GridPlacement::Span(style.column_span.max(1)),
            end: GridPlacement::Auto,
        },
        flex_wrap: if style.flex_wrap {
            taffy::FlexWrap::Wrap
        } else {
            taffy::FlexWrap::NoWrap
        },
        justify_content: Some(match style.justify {
            Justify::Start => JustifyContent::FlexStart,
            Justify::Center => JustifyContent::Center,
            Justify::End => JustifyContent::FlexEnd,
            Justify::SpaceBetween => JustifyContent::SpaceBetween,
        }),
        ..Default::default()
    }
}

struct Pending<'a> {
    node: &'a BoxNode,
    id: NodeId,
    children: Vec<Pending<'a>>,
}

struct Planner<'a, 'c> {
    cx: &'a LayoutCtx<'c>,
    column: f32,
    tree: TaffyTree<usize>,
    leaves: Vec<Leaf>,
    image_size: &'a mut dyn FnMut(&str) -> Option<(u32, u32)>,
}

impl<'a, 'c> Planner<'a, 'c> {
    fn plan(&mut self, node: &'a BoxNode) -> Pending<'a> {
        let style = taffy_style(&node.style, self.cx.env, self.column);
        let mut children = Vec::new();
        let id = match &node.content {
            Content::Container => {
                for child in &node.children {
                    children.push(self.plan(child));
                }
                let ids: Vec<NodeId> = children.iter().map(|c| c.id).collect();
                self.tree
                    .new_with_children(style, &ids)
                    .expect("taffy node")
            }
            Content::Text { text, .. } => {
                self.leaves.push(Leaf::Text {
                    text: text.clone(),
                    scale: self.cx.scale * node.style.font_scale,
                    line_height: self.cx.line_height * node.style.font_scale,
                });
                self.tree
                    .new_leaf_with_context(style, self.leaves.len() - 1)
                    .expect("taffy node")
            }
            Content::Media { path, poster, .. } => {
                let mut style = style;
                // A picture can be shown smaller than it is, so it never holds a
                // column open: columns of photos come out equal.
                style.min_size.width = length(0.0_f32);
                // And it is as tall as its shape says, not as tall as the row: a browser
                // does not stretch a replaced element that has an aspect ratio either.
                style.align_self = Some(taffy::AlignSelf::Start);
                let size = (self.image_size)(poster.as_deref().unwrap_or(path));
                let (aspect, natural_w) = match size {
                    Some((w, h)) if w > 0 && h > 0 => (w as f32 / h as f32, w as f32),
                    _ => (4.0 / 3.0, 12.0 * self.cx.env.base_ch),
                };
                let aspect = node.style.aspect_ratio.unwrap_or(aspect);
                self.leaves.push(Leaf::Media { aspect, natural_w });
                self.tree
                    .new_leaf_with_context(style, self.leaves.len() - 1)
                    .expect("taffy node")
            }
        };
        Pending { node, id, children }
    }
}

fn widest_line(fr: &FontRenderer, text: &str, scale: f32) -> f32 {
    text.split('\n')
        .map(|line| fr.measure_text_width(line, scale))
        .fold(0.0, f32::max)
}

fn longest_word(fr: &FontRenderer, text: &str, scale: f32) -> f32 {
    text.split_whitespace()
        .map(|word| fr.measure_text_width(word, scale))
        .fold(0.0, f32::max)
}

fn measure(
    fr: &FontRenderer,
    leaf: &Leaf,
    known: Size<Option<f32>>,
    available: Size<AvailableSpace>,
) -> Size<f32> {
    match leaf {
        Leaf::Text {
            text,
            scale,
            line_height,
        } => {
            let width = match (known.width, available.width) {
                (Some(w), _) => w,
                (None, AvailableSpace::Definite(w)) => w,
                (None, AvailableSpace::MinContent) => longest_word(fr, text, *scale),
                (None, AvailableSpace::MaxContent) => widest_line(fr, text, *scale),
            };
            let lines = fr.wrap_lines_with_offsets(text, *scale, width.max(1.0));
            let widest = lines
                .iter()
                .map(|(line, _)| fr.measure_text_width(line, *scale))
                .fold(0.0, f32::max);
            Size {
                width: known.width.unwrap_or(widest.min(width)),
                height: known
                    .height
                    .unwrap_or(lines.len().max(1) as f32 * line_height),
            }
        }
        Leaf::Media { aspect, natural_w } => match (known.width, known.height) {
            (Some(w), Some(h)) => Size {
                width: w,
                height: h,
            },
            (Some(w), None) => Size {
                width: w,
                height: w / aspect,
            },
            (None, Some(h)) => Size {
                width: h * aspect,
                height: h,
            },
            (None, None) => {
                let w = match available.width {
                    AvailableSpace::Definite(a) => natural_w.min(a),
                    // A picture can be shown smaller than it is, so it never holds a
                    // column open: `1fr` columns of photos come out equal.
                    AvailableSpace::MinContent => 0.0,
                    AvailableSpace::MaxContent => *natural_w,
                };
                Size {
                    width: w,
                    height: w / aspect,
                }
            }
        },
    }
}

struct Collector<'a, 'c> {
    cx: &'a LayoutCtx<'c>,
    tree: &'a TaffyTree<usize>,
    boxes: Vec<Placed>,
    targets: Vec<TargetRect>,
}

impl Collector<'_, '_> {
    fn collect(&mut self, pending: &Pending<'_>, origin: (f32, f32), behind: Option<u32>) {
        let layout = self.tree.layout(pending.id).expect("taffy layout");
        let (x, y) = (origin.0 + layout.location.x, origin.1 + layout.location.y);
        let (w, h) = (layout.size.width, layout.size.height);
        let node = pending.node;
        let cx = length_context(&node.style, self.cx.env, w);
        let px = |len: Length| len.px(&cx).unwrap_or(0.0);
        let border = px(node.style.border_width);
        let inset = [
            border + px(node.style.padding[0]),
            border + px(node.style.padding[1]),
            border + px(node.style.padding[2]),
            border + px(node.style.padding[3]),
        ];
        let content = match &node.content {
            Content::Container => PlacedContent::Container,
            Content::Text { text, runs } => {
                let scale = self.cx.scale * node.style.font_scale;
                let content_w = (w - inset[1] - inset[3]).max(1.0);
                let lines = self
                    .cx
                    .fr
                    .wrap_lines_with_offsets(text, scale, content_w)
                    .into_iter()
                    .map(|(line, offset)| (line, text[..offset].chars().count()))
                    .collect();
                PlacedContent::Text {
                    lines,
                    runs: runs.clone(),
                    scale,
                    line_height: self.cx.line_height * node.style.font_scale,
                }
            }
            Content::Media {
                path,
                is_video,
                name,
                poster,
            } => PlacedContent::Media {
                path: path.clone(),
                is_video: *is_video,
                name: name.clone(),
                poster: poster.clone(),
                fit: node.style.object_fit,
            },
        };
        let backdrop = node.style.background.or(behind);
        let rect = TargetRect { x, y, w, h };
        self.targets.extend(std::iter::repeat_n(rect, node.targets));
        self.boxes.push(Placed {
            x,
            y,
            w,
            h,
            style: node.style.clone(),
            inset,
            border,
            radius: px(node.style.border_radius),
            backdrop,
            content,
        });
        for child in &pending.children {
            self.collect(child, (x, y), backdrop);
        }
    }
}

/// Give every box of `root` a place in a row `width` pixels wide.
pub(crate) fn layout(
    root: &BoxNode,
    width: f32,
    cx: &LayoutCtx<'_>,
    image_size: &mut dyn FnMut(&str) -> Option<(u32, u32)>,
) -> RowLayout {
    let mut planner = Planner {
        cx,
        column: width,
        tree: TaffyTree::new(),
        leaves: Vec::new(),
        image_size,
    };
    let pending = planner.plan(root);
    let Planner {
        mut tree, leaves, ..
    } = planner;
    // The row is as wide as it is given, whatever its own style says: that is the
    // column, and `max-width` already narrowed it.
    let mut style = tree.style(pending.id).expect("root style").clone();
    style.size.width = length(width);
    let _ = tree.set_style(pending.id, style);
    let fr = cx.fr;
    let _ = tree.compute_layout_with_measure(
        pending.id,
        Size {
            width: AvailableSpace::Definite(width),
            height: AvailableSpace::MaxContent,
        },
        |known, available, _id, context, _style| match context {
            Some(&mut index) => measure(fr, &leaves[index], known, available),
            None => Size::ZERO,
        },
    );
    let mut collector = Collector {
        cx,
        tree: &tree,
        boxes: Vec::new(),
        targets: Vec::new(),
    };
    collector.collect(&pending, (0.0, 0.0), None);
    let height = collector.boxes.first().map_or(0.0, |root| root.h);
    RowLayout {
        width,
        height,
        boxes: collector.boxes,
        targets: collector.targets,
    }
}

/// What a laid out row depends on.
pub(crate) type BoxKey = (String, bool, i32, u32, u32, u32, u32, u8);

/// [`layout`] for a row of markup, kept until the stylesheets or the size change: a grid
/// of a hundred photos is laid out once, not once a frame.
pub(crate) fn layout_row(
    markup: &str,
    focus: bool,
    sheets: &Sheets,
    env: &StyleEnv,
    fr: &FontRenderer,
    scale: f32,
    line_height: f32,
    width: f32,
    image_size: &mut dyn FnMut(&str) -> Option<(u32, u32)>,
) -> std::sync::Arc<RowLayout> {
    let key: BoxKey = (
        markup.to_owned(),
        focus,
        width.round() as i32,
        scale.to_bits(),
        line_height.to_bits(),
        env.base_em.to_bits(),
        env.px_scale.to_bits(),
        env.scheme as u8,
    );
    if let Some(found) = sheets.box_cache.borrow().get(&key) {
        return std::sync::Arc::clone(found);
    }
    let root = build(markup, sheets, env, sheets.row_path(focus));
    let cx = LayoutCtx {
        fr,
        scale,
        line_height,
        env,
    };
    let laid = std::sync::Arc::new(layout(&root, width, &cx, image_size));
    let mut cache = sheets.box_cache.borrow_mut();
    if cache.len() > 256 {
        cache.clear();
    }
    cache.insert(key, std::sync::Arc::clone(&laid));
    laid
}

/// How far a row taller than the window is scrolled, so that the target at `target_y`
/// (height `target_h`) is on screen. `scroll` is where it was. The viewport is
/// `viewport` pixels of the row's `content_h`, and a little margin is kept around the
/// target so what is next to it shows too.
pub(crate) fn scroll_to_show(
    scroll: f32,
    target_y: f32,
    target_h: f32,
    viewport: f32,
    content_h: f32,
) -> f32 {
    let margin = 8.0_f32.min(viewport / 4.0);
    let mut next = scroll;
    if target_y - next < 0.0 {
        next = target_y - margin;
    } else if target_y + target_h - next > viewport {
        next = target_y + target_h - viewport + margin;
    }
    // A target taller than the viewport shows its top.
    if target_h > viewport {
        next = target_y;
    }
    next.clamp(0.0, (content_h - viewport).max(0.0))
}

/// Where a box's text starts and how wide it may be, in the row's pixels.
pub(crate) fn text_origin(placed: &Placed) -> (f32, f32, f32) {
    (
        placed.x + placed.inset[3],
        placed.y + placed.inset[0],
        (placed.w - placed.inset[1] - placed.inset[3]).max(1.0),
    )
}

/// Where a text box's line starts on the row, with the box's `text-align`.
pub(crate) fn line_x(placed: &Placed, fr: &FontRenderer, line: &str, scale: f32) -> f32 {
    let (x, _, width) = text_origin(placed);
    let free = (width - fr.measure_text_width(line, scale)).max(0.0);
    match placed.style.text_align {
        TextAlign::Left => x,
        TextAlign::Center => x + free / 2.0,
        TextAlign::Right => x + free,
    }
}

/// Whether a row of markup is one of boxes: it has a grid or flex container, a video,
/// or several images. A row of text with one picture is an ordinary image row.
pub(crate) fn needs_boxes(
    nodes: &[Node],
    sheets: &Sheets,
    env: &StyleEnv,
    row_path: &[StyleNode],
) -> bool {
    fn media(nodes: &[Node], images: &mut usize, videos: &mut usize) {
        for node in nodes {
            if let Node::Element(element) = node {
                match element.tag {
                    "img" => *images += 1,
                    "video" => *videos += 1,
                    _ => {}
                }
                media(&element.children, images, videos);
            }
        }
    }
    fn containers(
        nodes: &[Node],
        sheets: &Sheets,
        env: &StyleEnv,
        path: &mut Vec<StyleNode>,
    ) -> bool {
        nodes.iter().any(|node| {
            let Node::Element(element) = node else {
                return false;
            };
            path.push(node_of(element));
            let laid_out = sheets.style_for(path, env).display != Display::Block
                || containers(&element.children, sheets, env, path);
            path.pop();
            laid_out
        })
    }
    let (mut images, mut videos) = (0, 0);
    media(nodes, &mut images, &mut videos);
    videos > 0 || images > 1 || containers(nodes, sheets, env, &mut row_path.to_vec())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::text::make_fr_uniform;

    const LH: f32 = 20.0;

    fn env() -> StyleEnv {
        StyleEnv {
            base_em: 20.0,
            base_ch: 10.0,
            px_scale: 1.0,
            ..Default::default()
        }
    }

    fn sheets(css: &str) -> Sheets {
        let mut sheets = Sheets::default();
        sheets.set_user(Some(css));
        sheets
    }

    fn built(css: &str, row: &str) -> BoxNode {
        let sheets = sheets(css);
        build(row, &sheets, &env(), sheets.row_path(false))
    }

    /// 10 px a character, 20 px a line.
    fn laid(css: &str, row: &str, width: f32, sizes: &[(&str, (u32, u32))]) -> RowLayout {
        let sheets = sheets(css);
        let root = build(row, &sheets, &env(), sheets.row_path(false));
        let fr = make_fr_uniform(10.0);
        let env = env();
        let cx = LayoutCtx {
            fr: &fr,
            scale: 1.0,
            line_height: LH,
            env: &env,
        };
        let sizes = sizes.to_vec();
        layout(&root, width, &cx, &mut |path| {
            sizes.iter().find(|(p, _)| *p == path).map(|(_, s)| *s)
        })
    }

    fn media_boxes(layout: &RowLayout) -> Vec<&Placed> {
        layout
            .boxes
            .iter()
            .filter(|b| matches!(b.content, PlacedContent::Media { .. }))
            .collect()
    }

    /// The user's grid, in full.
    const GRID_CSS: &str = ".grid-container { display: grid; grid-template-columns: repeat(3, 1fr);
        grid-template-rows: auto; gap: 15px }
        .grid-item { background-color: #3498db; color: white; padding: 20px; text-align: center }";
    const GRID_ROW: &str = r#"<div class="grid-container">
        <div class="grid-item">1</div><div class="grid-item">2</div><div class="grid-item">3</div>
        <div class="grid-item">4</div><div class="grid-item">5</div><div class="grid-item">6</div>
    </div>"#;

    #[test]
    fn a_row_of_plain_text_is_one_text_block() {
        let root = built("", "<p>Hello there</p>");
        assert_eq!(root.children.len(), 1);
        assert!(
            matches!(&root.children[0].content, Content::Text { text, .. } if text == "Hello there")
        );
    }

    /// Three equal columns, two rows, 15 px between.
    #[test]
    fn the_grid_example_is_three_columns_of_equal_width_and_two_rows() {
        let layout = laid(GRID_CSS, GRID_ROW, 900.0, &[]);
        let items: Vec<&Placed> = layout
            .boxes
            .iter()
            .filter(|b| b.style.background == Some(0x3498DBFF))
            .collect();
        assert_eq!(items.len(), 6);
        // (900 - 2 * 15) / 3 = 290 wide.
        for item in &items {
            assert_eq!(item.w, 290.0, "{item:?}");
        }
        let xs: Vec<f32> = items.iter().map(|i| i.x).collect();
        assert_eq!(xs, [0.0, 305.0, 610.0, 0.0, 305.0, 610.0]);
        assert_eq!(items[0].y, 0.0);
        assert_eq!(items[3].y, items[0].h + 15.0);
        // 20 padding each side and one line of text.
        assert_eq!(items[0].h, 20.0 + LH + 20.0);
        assert_eq!(layout.height, 2.0 * items[0].h + 15.0);
    }

    /// Source order is what the keys follow, and the grid keeps it.
    #[test]
    fn the_grid_keeps_source_order() {
        let layout = laid(GRID_CSS, GRID_ROW, 900.0, &[]);
        let texts: Vec<String> = layout
            .boxes
            .iter()
            .filter_map(|b| match &b.content {
                PlacedContent::Text { lines, .. } => Some(lines[0].0.clone()),
                _ => None,
            })
            .collect();
        assert_eq!(texts, ["1", "2", "3", "4", "5", "6"]);
    }

    #[test]
    fn text_in_a_cell_is_centred_by_its_style() {
        let layout = laid(GRID_CSS, GRID_ROW, 900.0, &[]);
        let text = layout
            .boxes
            .iter()
            .find(|b| matches!(&b.content, PlacedContent::Text { lines, .. } if lines[0].0 == "1"))
            .expect("text");
        let fr = make_fr_uniform(10.0);
        let x = line_x(text, &fr, "1", 1.0);
        // The cell's text box is as wide as the cell inside its padding.
        assert!(x > text.x + text.inset[3], "{x} in {text:?}");
    }

    /// A photo album: images in a grid, each its shape, filling the column.
    #[test]
    fn images_fill_their_columns_and_keep_their_proportions() {
        let css = ".album { display: grid; grid-template-columns: repeat(2, 1fr); gap: 10px }";
        let row = r#"<div class="album"><img src="a.jpg"><img src="b.jpg"><img src="c.jpg"></div>"#;
        let layout = laid(
            css,
            row,
            410.0,
            &[
                ("a.jpg", (400, 200)),
                ("b.jpg", (300, 300)),
                ("c.jpg", (100, 100)),
            ],
        );
        let media = media_boxes(&layout);
        assert_eq!(media.len(), 3);
        // (410 - 10) / 2 = 200 wide.
        assert_eq!((media[0].w, media[0].h), (200.0, 100.0));
        assert_eq!((media[1].w, media[1].h), (200.0, 200.0));
        assert_eq!((media[1].x, media[0].x), (210.0, 0.0));
        // The next row starts under the taller of the first two, with the gap.
        assert_eq!((media[2].x, media[2].y), (0.0, 210.0));
    }

    #[test]
    fn a_set_aspect_ratio_makes_thumbnails_the_same_shape() {
        let css = ".g { display: grid; grid-template-columns: 1fr 1fr } img { aspect-ratio: 1; object-fit: cover }";
        let layout = laid(
            css,
            r#"<div class="g"><img src="a.jpg"><img src="b.jpg"></div>"#,
            200.0,
            &[("a.jpg", (400, 100)), ("b.jpg", (100, 400))],
        );
        for media in media_boxes(&layout) {
            assert_eq!((media.w, media.h), (100.0, 100.0));
            assert!(matches!(
                &media.content,
                PlacedContent::Media {
                    fit: ObjectFit::Cover,
                    ..
                }
            ));
        }
    }

    #[test]
    fn an_image_that_has_not_loaded_still_gets_a_place() {
        let layout = laid("", r#"<img src="a.jpg"><img src="b.jpg">"#, 300.0, &[]);
        for media in media_boxes(&layout) {
            assert!(media.w > 0.0 && media.h > 0.0, "{media:?}");
        }
    }

    /// Without a grid, pictures stack, each as wide as the row allows.
    #[test]
    fn several_images_in_a_plain_row_stack() {
        let layout = laid(
            "",
            r#"<img src="a.jpg"><img src="b.jpg">"#,
            300.0,
            &[("a.jpg", (600, 300)), ("b.jpg", (600, 600))],
        );
        let media = media_boxes(&layout);
        assert_eq!(
            (media[0].x, media[0].y, media[0].w, media[0].h),
            (0.0, 0.0, 300.0, 150.0)
        );
        assert_eq!((media[1].y, media[1].w, media[1].h), (150.0, 300.0, 300.0));
        assert_eq!(layout.height, 450.0);
    }

    #[test]
    fn flex_puts_thumbnails_side_by_side_and_wraps() {
        let css = ".row { display: flex; flex-wrap: wrap; gap: 10px } img { width: 100px }";
        let row = r#"<div class="row"><img src="a.jpg"><img src="b.jpg"><img src="c.jpg"></div>"#;
        let sizes = [
            ("a.jpg", (100, 100)),
            ("b.jpg", (100, 100)),
            ("c.jpg", (100, 100)),
        ];
        let layout = laid(css, row, 230.0, &sizes);
        let media = media_boxes(&layout);
        assert_eq!((media[0].x, media[1].x), (0.0, 110.0));
        // The third no longer fits on the first line.
        assert_eq!((media[2].x, media[2].y), (0.0, 110.0));
    }

    #[test]
    fn a_style_on_the_row_element_is_the_boxs_own() {
        let layout = laid(
            ".card { padding: 10px; border: 2px solid #fff; border-radius: 6px }",
            r#"<div class="card">Hi</div>"#,
            300.0,
            &[],
        );
        let card = &layout.boxes[0];
        assert_eq!(card.inset, [12.0; 4]);
        assert_eq!((card.border, card.radius), (2.0, 6.0));
        assert_eq!(card.h, LH + 24.0);
    }

    #[test]
    fn text_wraps_at_the_width_it_is_given() {
        // "aaaa bbbb" and the space after it fill 100 px.
        let layout = laid("", "<p>aaaa bbbb cccc</p>", 100.0, &[]);
        let PlacedContent::Text { lines, .. } = &layout
            .boxes
            .iter()
            .find(|b| matches!(b.content, PlacedContent::Text { .. }))
            .expect("text")
            .content
        else {
            unreachable!()
        };
        assert_eq!(lines.len(), 2, "{lines:?}");
        assert_eq!(lines[1].1, 10, "the second line starts at character 10");
        assert_eq!(layout.height, 2.0 * LH);
    }

    #[test]
    fn a_bigger_font_takes_bigger_lines() {
        let layout = laid("p { font-size: 2em }", "<p>one</p>", 300.0, &[]);
        assert_eq!(layout.height, 2.0 * LH);
    }

    #[test]
    fn inline_styles_are_kept_as_runs_at_the_right_characters() {
        let root = built("b { color: #e80 }", "<p>one <b>two</b> three</p>");
        let Content::Text { runs, .. } = &root.children[0].content else {
            panic!("{root:?}")
        };
        assert_eq!(runs.len(), 1);
        assert_eq!((runs[0].start, runs[0].end), (4, 7));
    }

    #[test]
    fn leading_space_and_newlines_do_not_move_the_runs() {
        let root = built("", "<div>\n  <b>x</b>\n</div>");
        let Content::Text { text, runs } = &root.children[0].content else {
            panic!("{root:?}")
        };
        assert_eq!(text, "x");
        assert_eq!((runs[0].start, runs[0].end), (0, 1));
    }

    /// Every target is somewhere, in the order the row lists them.
    #[test]
    fn targets_are_numbered_the_way_the_row_numbers_them() {
        let row = r#"<div class="g"><img src="a.jpg" alt="A"><img src="b.jpg"><p>See <a href="u">home</a></p></div>"#;
        let layout = laid(
            ".g { display: grid; grid-template-columns: 1fr 1fr }",
            row,
            400.0,
            &[],
        );
        let want = markup::targets(row);
        assert_eq!(layout.targets.len(), want.len());
        assert_eq!(want.len(), 3);
        // The two images are on the first line, the link below them.
        assert_eq!(layout.targets[0].y, layout.targets[1].y);
        assert!(layout.targets[2].y > layout.targets[0].y);
        assert!(layout.targets[1].x > layout.targets[0].x);
    }

    #[test]
    fn a_link_around_a_picture_shares_the_pictures_box() {
        let row = r#"<a href="page"><img src="t.jpg" alt="Trip"></a> after"#;
        let layout = laid("", row, 300.0, &[("t.jpg", (100, 100))]);
        assert_eq!(markup::targets(row).len(), 2);
        assert_eq!(layout.targets.len(), 2);
        assert_eq!(layout.targets[0], layout.targets[1]);
    }

    #[test]
    fn a_video_is_a_target_and_a_media_box() {
        let row = r#"<video src="v.mp4" poster="p.jpg">Clip</video>"#;
        let layout = laid("", row, 300.0, &[("p.jpg", (640, 360))]);
        let media = media_boxes(&layout);
        assert_eq!(media.len(), 1);
        // taffy puts everything on whole pixels.
        assert_eq!((media[0].w, media[0].h), (300.0, 169.0));
        assert!(
            matches!(&media[0].content, PlacedContent::Media { is_video: true, path, name, .. } if path == "v.mp4" && name == "Clip")
        );
        assert_eq!(layout.targets.len(), 1);
    }

    #[test]
    fn counts_of_targets_always_agree_with_the_row() {
        for row in [
            r#"<div><img src="a"><img src="b"></div>"#,
            r#"<div><p>x <a href="u">y</a> <button>f</button>Go</p><img src="a"></div>"#,
            r#"<div><a href="u"><img src="a"></a><a href="v"><img src="b"></a></div>"#,
            r#"<span><b><img src="a"></b></span> tail <link>u</link>"#,
            r#"<video src="v.mp4"></video><p>caption <a href="u">go</a></p>"#,
            r#"<div><div><img src="a"></div><div><img src="b"><p>c</p></div></div>"#,
        ] {
            let layout = laid("", row, 400.0, &[]);
            assert_eq!(layout.targets.len(), markup::targets(row).len(), "{row}");
        }
    }

    #[test]
    fn scrolling_follows_the_target_and_stops_at_the_ends() {
        // A 1000 px row seen through 300 px.
        assert_eq!(
            scroll_to_show(0.0, 50.0, 100.0, 300.0, 1000.0),
            0.0,
            "already visible"
        );
        assert_eq!(
            scroll_to_show(0.0, 400.0, 100.0, 300.0, 1000.0),
            208.0,
            "below: its bottom plus a margin"
        );
        assert_eq!(
            scroll_to_show(500.0, 100.0, 100.0, 300.0, 1000.0),
            92.0,
            "above: its top less a margin"
        );
        assert_eq!(
            scroll_to_show(0.0, 900.0, 100.0, 300.0, 1000.0),
            700.0,
            "never past the end"
        );
        assert_eq!(
            scroll_to_show(500.0, 0.0, 100.0, 300.0, 1000.0),
            0.0,
            "never before the start"
        );
        assert_eq!(
            scroll_to_show(700.0, 750.0, 100.0, 300.0, 1000.0),
            700.0,
            "visible stays put"
        );
    }

    #[test]
    fn a_row_that_fits_never_scrolls_and_a_huge_target_shows_its_top() {
        assert_eq!(scroll_to_show(0.0, 90.0, 10.0, 300.0, 100.0), 0.0);
        assert_eq!(scroll_to_show(0.0, 500.0, 900.0, 300.0, 2000.0), 500.0);
    }

    #[test]
    fn a_box_knows_what_is_behind_it() {
        let layout = laid(
            ".card { background: #112233 } .card p { color: white }",
            r#"<div class="card"><p>x</p></div>"#,
            200.0,
            &[],
        );
        assert_eq!(layout.boxes[0].backdrop, Some(0x112233FF));
        assert_eq!(layout.boxes[1].backdrop, Some(0x112233FF));
        let none = laid("", "<div><p>x</p></div>", 200.0, &[]);
        assert_eq!(none.boxes[1].backdrop, None);
    }

    #[test]
    fn a_row_decides_whether_it_needs_boxes() {
        let sheets = sheets(".g { display: grid }");
        let env = env();
        let path = sheets.row_path(false);
        let needs = |row: &str| needs_boxes(&markup::parse(row), &sheets, &env, &path);
        assert!(needs(r#"<div class="g">x</div>"#), "a grid");
        assert!(needs(r#"<img src="a"><img src="b">"#), "two pictures");
        assert!(needs(r#"<video src="v"></video>"#), "a video");
        assert!(!needs(r#"<img src="a">"#), "one picture is an image row");
        assert!(!needs("<p>text <b>bold</b></p>"));
        assert!(!needs("plain"));
    }

    #[test]
    fn a_laid_out_row_is_kept_until_something_changes() {
        let sheets = sheets(GRID_CSS);
        let env = env();
        let fr = make_fr_uniform(10.0);
        let looked_up = std::cell::Cell::new(0);
        let mut size = |_: &str| {
            looked_up.set(looked_up.get() + 1);
            Some((100, 100))
        };
        let row = r#"<div class="grid-container"><img src="a"><img src="b"></div>"#;
        let once = layout_row(row, false, &sheets, &env, &fr, 1.0, LH, 600.0, &mut size);
        let again = layout_row(row, false, &sheets, &env, &fr, 1.0, LH, 600.0, &mut size);
        assert!(std::sync::Arc::ptr_eq(&once, &again));
        assert_eq!(looked_up.get(), 2, "each picture's size was asked for once");
        let narrower = layout_row(row, false, &sheets, &env, &fr, 1.0, LH, 300.0, &mut size);
        assert!(!std::sync::Arc::ptr_eq(&once, &narrower));
        assert_ne!(once.height, narrower.height);
        let focused = layout_row(row, true, &sheets, &env, &fr, 1.0, LH, 600.0, &mut size);
        assert!(
            !std::sync::Arc::ptr_eq(&once, &focused),
            "li:focus can change it"
        );
    }

    #[test]
    fn a_new_stylesheet_forgets_the_laid_out_rows() {
        let mut sheets = sheets(".g { display: grid; grid-template-columns: 1fr 1fr }");
        let env = env();
        let fr = make_fr_uniform(10.0);
        let row = r#"<div class="g"><img src="a"><img src="b"></div>"#;
        let mut size = |_: &str| Some((100, 100));
        let before = layout_row(row, false, &sheets, &env, &fr, 1.0, LH, 400.0, &mut size);
        assert_eq!(before.boxes[1].x, 0.0);
        sheets.set_user(Some(
            ".g { display: grid; grid-template-columns: 1fr 1fr 1fr }",
        ));
        let after = layout_row(row, false, &sheets, &env, &fr, 1.0, LH, 400.0, &mut size);
        assert_ne!(before.boxes[2].w, after.boxes[2].w);
    }

    #[test]
    fn the_row_is_as_wide_as_it_is_given() {
        let layout = laid("div { width: 10px }", "<div>x</div>", 250.0, &[]);
        assert_eq!(layout.boxes[0].w, 250.0);
        assert_eq!(layout.width, 250.0);
    }

    #[test]
    fn an_empty_row_has_no_height_and_does_not_crash() {
        let layout = laid("", "<div></div>", 100.0, &[]);
        assert_eq!(layout.height, 0.0);
        assert!(layout.targets.is_empty());
    }
}
