//! Real CSS syntax for the lists, a subset, drawn by this renderer.
//!
//! Selectors and tokens are the real thing (`cssparser` and `selectors`, which
//! `scraper` already brings into every binary that links the SDK). The tree they
//! match against is what a screen is: a `ul`, its `li` rows, and what is inside a
//! row. `li:focus` is the row or target the cursor is on, and `li::marker` is the
//! list prefix.
//!
//! The cascade is: [`DEFAULT_CSS`], then a provider's or site's sheet, then the
//! user's `user.css`, in that order, each ordered by specificity within itself.
//! Accessibility settings are not in the cascade at all. They sit on top of what it
//! computes: font scale through the length units, and contrast through
//! [`readable_color`].
//!
//! Source order is what the keys and the screen reader follow, always. So anything
//! that would make the screen disagree with them is not a property here:
//! `display: none`, `order`, `content`, reversed flex, a positioned grid item. An
//! unknown property is ignored and logged once, the way a browser ignores it. Nothing
//! here reaches the network: there is no `url()`.

use crate::layout::{RowStyle, TextRun};
use cssparser::{ParseError, Parser as CssParser, ParserInput, ToCss, Token};
use precomputed_hash::PrecomputedHash;
use selectors::attr::{AttrSelectorOperation, CaseSensitivity, NamespaceConstraint};
use selectors::bloom::BloomFilter;
use selectors::matching::{
    self, ElementSelectorFlags, MatchingContext, MatchingForInvalidation, MatchingMode,
    NeedsSelectorFlags, QuirksMode, SelectorCaches,
};
use selectors::parser::{self, ParseRelative, Selector, SelectorList, SelectorParseErrorKind};
use selectors::{Element, OpaqueElement};
use sicompass_sdk::ffon::IdArray;
use sicompass_sdk::{markup, tags};
use std::cell::RefCell;
use std::collections::{HashMap, HashSet};
use std::fmt;
use std::sync::{Arc, Mutex};

/// What every list starts from. The column every current plugin is drawn in used to
/// be hardcoded in the view: a site or a `user.css` goes wide with
/// `ul { max-width: none }`.
/// The corners of every background, until a stylesheet says otherwise.
pub const DEFAULT_RADIUS: f32 = 5.0;

// Colours are written the way the palette writes them, not as they look on screen: the
// swapchain is sRGB, so they come out lighter. `link` and `button` below are the same.
// A button is the palette's highlight blue (`scroll_search`). In element mode the view
// quietens the ones the cursor is not on, so the one it is on stands out.
pub const DEFAULT_CSS: &str = "ul { max-width: 120ch }
    h1 { font-size: 2em; font-weight: bold }
    h2 { font-size: 1.5em; font-weight: bold }
    h3 { font-size: 1.17em; font-weight: bold }
    header { font-weight: bold }
    b { font-weight: bold }
    a, button { font-weight: bold }
    @media (prefers-color-scheme: dark) { a { color: #267AFF } button { color: #FFFFFF; background: #264F78 } }
    @media (prefers-color-scheme: light) { a { color: #001B6D } button { color: #000000; background: #A8C7FA } }";

// ---------------------------------------------------------------------------
// Values
// ---------------------------------------------------------------------------

/// A length as written.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Length {
    Px(f32),
    Em(f32),
    Ch(f32),
    Percent(f32),
    /// A grid track's share of the free room.
    Fr(f32),
    Auto,
}

/// What a [`Length`] is measured against.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct LengthContext {
    /// The element's font size, in pixels.
    pub em: f32,
    /// The width of a character at that size, in pixels.
    pub ch: f32,
    /// What 100% is, in pixels.
    pub percent_of: f32,
    /// Pixels per CSS px, so a stylesheet's sizes grow with the font-scale setting.
    pub px_scale: f32,
}

impl Length {
    /// In pixels. `Auto` and `Fr` have no size of their own, so they are `None`.
    pub fn px(self, cx: &LengthContext) -> Option<f32> {
        match self {
            Length::Px(v) => Some(v * cx.px_scale),
            Length::Em(v) => Some(v * cx.em),
            Length::Ch(v) => Some(v * cx.ch),
            Length::Percent(v) => Some(v / 100.0 * cx.percent_of),
            Length::Fr(_) | Length::Auto => None,
        }
    }
}

/// `top right bottom left`.
pub type Sides = [Length; 4];

const NO_SIDES: Sides = [Length::Px(0.0); 4];

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum TextAlign {
    #[default]
    Left,
    Center,
    Right,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Display {
    #[default]
    Block,
    Grid,
    Flex,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ObjectFit {
    #[default]
    Contain,
    Cover,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Justify {
    #[default]
    Start,
    Center,
    End,
    SpaceBetween,
}

/// The colour scheme, for `@media (prefers-color-scheme: ...)`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum Scheme {
    #[default]
    Dark,
    Light,
}

/// What the cascade computes for one element. A colour left `None` is the palette's.
#[derive(Debug, Clone, PartialEq)]
pub struct Style {
    pub color: Option<u32>,
    pub background: Option<u32>,
    /// Relative to the app's font size, so 1.0 is the size everything else is.
    pub font_scale: f32,
    pub bold: bool,
    pub padding: Sides,
    pub margin: Sides,
    pub border_width: Length,
    pub border_color: Option<u32>,
    pub border_radius: Length,
    pub width: Length,
    pub height: Length,
    /// `None` is `max-width: none`.
    pub max_width: Option<Length>,
    pub text_align: TextAlign,
    pub display: Display,
    pub grid_columns: Vec<Length>,
    pub grid_rows: Vec<Length>,
    pub gap: Length,
    pub column_span: u16,
    pub object_fit: ObjectFit,
    /// Width over height.
    pub aspect_ratio: Option<f32>,
    pub flex_wrap: bool,
    pub justify: Justify,
}

impl Default for Style {
    fn default() -> Self {
        Style {
            color: None,
            background: None,
            font_scale: 1.0,
            bold: false,
            padding: NO_SIDES,
            margin: NO_SIDES,
            border_width: Length::Px(0.0),
            border_color: None,
            border_radius: Length::Px(DEFAULT_RADIUS),
            width: Length::Auto,
            height: Length::Auto,
            max_width: None,
            text_align: TextAlign::Left,
            display: Display::Block,
            grid_columns: Vec::new(),
            grid_rows: Vec::new(),
            gap: Length::Px(0.0),
            column_span: 1,
            object_fit: ObjectFit::Contain,
            aspect_ratio: None,
            flex_wrap: false,
            justify: Justify::Start,
        }
    }
}

impl Style {
    /// What a child starts from: CSS's inherited properties, and nothing else.
    pub(crate) fn inherited(&self) -> Style {
        Style {
            color: self.color,
            font_scale: self.font_scale,
            bold: self.bold,
            text_align: self.text_align,
            ..Style::default()
        }
    }
}

// ---------------------------------------------------------------------------
// The tree selectors match against
// ---------------------------------------------------------------------------

/// One element on the way from the `ul` down.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct StyleNode {
    pub tag: String,
    pub classes: Vec<String>,
    /// `:focus`: the cursor is on it.
    pub focus: bool,
}

impl StyleNode {
    pub fn new(tag: &str) -> Self {
        StyleNode {
            tag: tag.to_owned(),
            ..Default::default()
        }
    }
}

/// The synthetic child a `::marker` selector is rewritten to, because a list prefix
/// is not a real element. `selectors` would make it a pseudo-element, whose matching
/// takes another mode and adds nothing here.
pub const MARKER_TAG: &str = "-sicompass-marker";

/// A name in a selector.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct CssName(String);

impl From<&str> for CssName {
    fn from(name: &str) -> Self {
        CssName(name.to_owned())
    }
}

impl AsRef<str> for CssName {
    fn as_ref(&self) -> &str {
        &self.0
    }
}

impl ToCss for CssName {
    fn to_css<W: fmt::Write>(&self, dest: &mut W) -> fmt::Result {
        dest.write_str(&self.0)
    }
}

impl PrecomputedHash for CssName {
    /// FNV-1a. Only feeds the bloom filter, which this tree does not use.
    fn precomputed_hash(&self) -> u32 {
        self.0
            .bytes()
            .fold(0x811c_9dc5, |h, b| (h ^ b as u32).wrapping_mul(0x0100_0193))
    }
}

/// `:focus`, the one state a list has.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PseudoClass {
    Focus,
}

impl parser::NonTSPseudoClass for PseudoClass {
    type Impl = Css;
    fn is_active_or_hover(&self) -> bool {
        false
    }
    fn is_user_action_state(&self) -> bool {
        true
    }
}

impl ToCss for PseudoClass {
    fn to_css<W: fmt::Write>(&self, dest: &mut W) -> fmt::Result {
        dest.write_str(":focus")
    }
}

/// Never constructed: see [`MARKER_TAG`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PseudoElement {}

impl parser::PseudoElement for PseudoElement {
    type Impl = Css;
}

impl ToCss for PseudoElement {
    fn to_css<W: fmt::Write>(&self, _dest: &mut W) -> fmt::Result {
        Ok(())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Css;

impl parser::SelectorImpl for Css {
    type AttrValue = CssName;
    type Identifier = CssName;
    type LocalName = CssName;
    type NamespacePrefix = CssName;
    type NamespaceUrl = CssName;
    type BorrowedNamespaceUrl = CssName;
    type BorrowedLocalName = CssName;
    type NonTSPseudoClass = PseudoClass;
    type PseudoElement = PseudoElement;
    type ExtraMatchingData<'a> = ();
}

struct SelectorParser;

impl<'i> parser::Parser<'i> for SelectorParser {
    type Impl = Css;
    type Error = SelectorParseErrorKind<'i>;

    fn parse_non_ts_pseudo_class(
        &self,
        location: cssparser::SourceLocation,
        name: cssparser::CowRcStr<'i>,
    ) -> Result<PseudoClass, ParseError<'i, Self::Error>> {
        if name.eq_ignore_ascii_case("focus") {
            Ok(PseudoClass::Focus)
        } else {
            Err(
                location.new_custom_error(SelectorParseErrorKind::UnsupportedPseudoClassOrElement(
                    name,
                )),
            )
        }
    }
}

/// An element: where it is on the path from the `ul`.
#[derive(Debug, Clone, Copy)]
struct PathElement<'a> {
    path: &'a [StyleNode],
    index: usize,
}

impl PathElement<'_> {
    fn node(&self) -> &StyleNode {
        &self.path[self.index]
    }
}

impl Element for PathElement<'_> {
    type Impl = Css;

    fn opaque(&self) -> OpaqueElement {
        OpaqueElement::new(self.node())
    }
    fn parent_element(&self) -> Option<Self> {
        self.index.checked_sub(1).map(|index| PathElement {
            path: self.path,
            index,
        })
    }
    fn parent_node_is_shadow_root(&self) -> bool {
        false
    }
    fn containing_shadow_host(&self) -> Option<Self> {
        None
    }
    fn is_pseudo_element(&self) -> bool {
        false
    }
    fn is_part(&self, _name: &CssName) -> bool {
        false
    }
    fn is_same_type(&self, other: &Self) -> bool {
        self.node().tag == other.node().tag
    }
    fn imported_part(&self, _name: &CssName) -> Option<CssName> {
        None
    }
    fn prev_sibling_element(&self) -> Option<Self> {
        None
    }
    fn next_sibling_element(&self) -> Option<Self> {
        None
    }
    fn first_element_child(&self) -> Option<Self> {
        self.path.get(self.index + 1).map(|_| PathElement {
            path: self.path,
            index: self.index + 1,
        })
    }
    fn is_html_element_in_html_document(&self) -> bool {
        true
    }
    fn has_local_name(&self, name: &CssName) -> bool {
        // Written the web's way, these are our elements.
        let tag = self.node().tag.as_str();
        tag == name.0
            || match name.0.as_str() {
                "img" => tag == "image",
                "a" => tag == "link",
                _ => false,
            }
    }
    fn has_namespace(&self, _ns: &CssName) -> bool {
        true
    }
    fn attr_matches(
        &self,
        _ns: &NamespaceConstraint<&CssName>,
        _name: &CssName,
        _op: &AttrSelectorOperation<&CssName>,
    ) -> bool {
        false
    }
    fn match_non_ts_pseudo_class(
        &self,
        pc: &PseudoClass,
        _cx: &mut MatchingContext<'_, Css>,
    ) -> bool {
        match pc {
            PseudoClass::Focus => self.node().focus,
        }
    }
    fn match_pseudo_element(&self, pe: &PseudoElement, _cx: &mut MatchingContext<'_, Css>) -> bool {
        match *pe {}
    }
    fn apply_selector_flags(&self, _flags: ElementSelectorFlags) {}
    fn is_link(&self) -> bool {
        self.node().tag == "link"
    }
    fn is_html_slot_element(&self) -> bool {
        false
    }
    fn has_id(&self, _id: &CssName, _cs: CaseSensitivity) -> bool {
        false
    }
    fn has_class(&self, name: &CssName, cs: CaseSensitivity) -> bool {
        self.node()
            .classes
            .iter()
            .any(|class| cs.eq(class.as_bytes(), name.0.as_bytes()))
    }
    fn has_custom_state(&self, _name: &CssName) -> bool {
        false
    }
    fn is_empty(&self) -> bool {
        self.index + 1 == self.path.len()
    }
    fn is_root(&self) -> bool {
        self.index == 0
    }
    fn add_element_unique_hashes(&self, _filter: &mut BloomFilter) -> bool {
        false
    }
}

// ---------------------------------------------------------------------------
// Declarations
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq)]
enum Declaration {
    Color(u32),
    Background(u32),
    FontSize(Length),
    Bold(bool),
    Padding(Sides),
    Margin(Sides),
    Border(Option<Length>, Option<u32>),
    BorderRadius(Length),
    Width(Length),
    Height(Length),
    MaxWidth(Option<Length>),
    TextAlign(TextAlign),
    Display(Display),
    GridColumns(Vec<Length>),
    GridRows(Vec<Length>),
    Gap(Length),
    ColumnSpan(u16),
    ObjectFit(ObjectFit),
    AspectRatio(f32),
    FlexWrap(bool),
    Justify(Justify),
}

impl Declaration {
    fn apply(&self, style: &mut Style, env: &StyleEnv, parent_font: f32) {
        match self {
            Declaration::Color(c) => style.color = Some(*c),
            Declaration::Background(c) => style.background = Some(*c),
            Declaration::FontSize(len) => {
                // Against the parent, as CSS has it, and kept as a ratio to the app's
                // font, so it follows the font-scale setting.
                let cx = LengthContext {
                    em: parent_font * env.base_em,
                    ch: parent_font * env.base_ch,
                    percent_of: parent_font * env.base_em,
                    px_scale: env.px_scale,
                };
                if let Some(px) = len.px(&cx) {
                    style.font_scale = px / env.base_em.max(1.0);
                }
            }
            Declaration::Bold(b) => style.bold = *b,
            Declaration::Padding(s) => style.padding = *s,
            Declaration::Margin(s) => style.margin = *s,
            Declaration::Border(width, color) => {
                if let Some(width) = width {
                    style.border_width = *width;
                }
                if let Some(color) = color {
                    style.border_color = Some(*color);
                }
            }
            Declaration::BorderRadius(l) => style.border_radius = *l,
            Declaration::Width(l) => style.width = *l,
            Declaration::Height(l) => style.height = *l,
            Declaration::MaxWidth(l) => style.max_width = *l,
            Declaration::TextAlign(a) => style.text_align = *a,
            Declaration::Display(d) => style.display = *d,
            Declaration::GridColumns(t) => style.grid_columns = t.clone(),
            Declaration::GridRows(t) => style.grid_rows = t.clone(),
            Declaration::Gap(l) => style.gap = *l,
            Declaration::ColumnSpan(n) => style.column_span = *n,
            Declaration::ObjectFit(f) => style.object_fit = *f,
            Declaration::AspectRatio(r) => style.aspect_ratio = Some(*r),
            Declaration::FlexWrap(w) => style.flex_wrap = *w,
            Declaration::Justify(j) => style.justify = *j,
        }
    }
}

type ValueResult<'i, T> = Result<T, ParseError<'i, ()>>;

fn number_of(input: &mut CssParser<'_, '_>) -> Option<f32> {
    match input.next().ok()? {
        Token::Number { value, .. } => Some(*value),
        Token::Percentage { unit_value, .. } => Some(*unit_value * 255.0),
        _ => None,
    }
}

const NAMED_COLORS: &[(&str, u32)] = &[
    ("transparent", 0x0000_0000),
    ("black", 0x0000_00FF),
    ("white", 0xFFFF_FFFF),
    ("red", 0xFF00_00FF),
    ("green", 0x0080_00FF),
    ("blue", 0x0000_FFFF),
    ("yellow", 0xFFFF_00FF),
    ("orange", 0xFFA5_00FF),
    ("purple", 0x8000_80FF),
    ("gray", 0x8080_80FF),
    ("grey", 0x8080_80FF),
    ("silver", 0xC0C0_C0FF),
    ("navy", 0x0000_80FF),
    ("teal", 0x0080_80FF),
    ("maroon", 0x8000_00FF),
];

fn hex_color(hex: &str) -> Option<u32> {
    if !hex.is_ascii() {
        return None;
    }
    let digits = |s: &str| u32::from_str_radix(s, 16).ok();
    let doubled = |s: &str| s.chars().flat_map(|c| [c, c]).collect::<String>();
    match hex.len() {
        3 => digits(&doubled(hex)).map(|v| (v << 8) | 0xFF),
        4 => digits(&doubled(hex)),
        6 => digits(hex).map(|v| (v << 8) | 0xFF),
        8 => digits(hex),
        _ => None,
    }
}

fn parse_color<'i>(input: &mut CssParser<'i, '_>) -> ValueResult<'i, u32> {
    let location = input.current_source_location();
    let token = input.next()?.clone();
    let color = match &token {
        Token::Hash(h) | Token::IDHash(h) => hex_color(h),
        Token::Ident(name) => NAMED_COLORS
            .iter()
            .find(|(n, _)| name.eq_ignore_ascii_case(n))
            .map(|(_, c)| *c),
        Token::Function(name)
            if name.eq_ignore_ascii_case("rgb") || name.eq_ignore_ascii_case("rgba") =>
        {
            input
                .parse_nested_block(|args| {
                    let part = |args: &mut CssParser<'i, '_>| -> Option<f32> {
                        let _ = args.try_parse(|a| a.expect_comma());
                        number_of(args)
                    };
                    let (r, g, b) = (part(args), part(args), part(args));
                    let _ = args.try_parse(|a| a.expect_delim('/'));
                    let alpha = args
                        .try_parse(|a| {
                            let _ = a.try_parse(|a| a.expect_comma());
                            a.expect_number()
                        })
                        .unwrap_or(1.0);
                    match (r, g, b) {
                        (Some(r), Some(g), Some(b)) => {
                            let byte = |v: f32| (v.clamp(0.0, 255.0).round() as u32) & 0xFF;
                            Ok(byte(r) << 24 | byte(g) << 16 | byte(b) << 8 | byte(alpha * 255.0))
                        }
                        _ => Err(args.new_custom_error::<_, ()>(())),
                    }
                })
                .ok()
        }
        _ => None,
    };
    color.ok_or_else(|| location.new_custom_error(()))
}

fn parse_length<'i>(input: &mut CssParser<'i, '_>) -> ValueResult<'i, Length> {
    let location = input.current_source_location();
    let length = match input.next()? {
        Token::Dimension { value, unit, .. } => match unit.to_ascii_lowercase().as_str() {
            "px" => Some(Length::Px(*value)),
            "em" | "rem" => Some(Length::Em(*value)),
            "ch" => Some(Length::Ch(*value)),
            "fr" => Some(Length::Fr(*value)),
            _ => None,
        },
        Token::Percentage { unit_value, .. } => Some(Length::Percent(*unit_value * 100.0)),
        Token::Number { value, .. } if *value == 0.0 => Some(Length::Px(0.0)),
        Token::Ident(name) if name.eq_ignore_ascii_case("auto") => Some(Length::Auto),
        _ => None,
    };
    length.ok_or_else(|| location.new_custom_error(()))
}

/// One to four lengths, CSS's way of writing sides.
fn parse_sides<'i>(input: &mut CssParser<'i, '_>) -> ValueResult<'i, Sides> {
    let mut values = vec![parse_length(input)?];
    while values.len() < 4 {
        match input.try_parse(parse_length) {
            Ok(l) => values.push(l),
            Err(_) => break,
        }
    }
    Ok(match values[..] {
        [a] => [a, a, a, a],
        [v, h] => [v, h, v, h],
        [t, h, b] => [t, h, b, h],
        [t, r, b, l] => [t, r, b, l],
        _ => NO_SIDES,
    })
}

/// A track list: lengths, `auto`, `fr`, and `repeat(3, 1fr)`.
fn parse_tracks<'i>(input: &mut CssParser<'i, '_>) -> ValueResult<'i, Vec<Length>> {
    let mut tracks = Vec::new();
    while !input.is_exhausted() {
        let repeated = input.try_parse(|i| {
            i.expect_function_matching("repeat")?;
            i.parse_nested_block(|args| {
                let count = args.expect_integer()?;
                args.expect_comma()?;
                let mut inner = Vec::new();
                while !args.is_exhausted() {
                    inner.push(parse_length(args)?);
                }
                Ok((count, inner))
            })
        });
        match repeated {
            Ok((count, inner)) => {
                // A hand-written number, but one that sizes an allocation.
                for _ in 0..count.clamp(0, 256) {
                    tracks.extend_from_slice(&inner);
                }
            }
            Err(_) => tracks.push(parse_length(input)?),
        }
    }
    Ok(tracks)
}

fn parse_border<'i>(input: &mut CssParser<'i, '_>) -> ValueResult<'i, Declaration> {
    let (mut width, mut color) = (None, None);
    while !input.is_exhausted() {
        if let Ok(l) = input.try_parse(parse_length) {
            width = Some(l);
        } else if let Ok(c) = input.try_parse(parse_color) {
            color = Some(c);
        } else {
            // `solid` and the other line styles: a border here is a solid one.
            input.next()?;
        }
    }
    Ok(Declaration::Border(width, color))
}

fn parse_keyword<'i, T: Copy>(
    input: &mut CssParser<'i, '_>,
    table: &[(&str, T)],
) -> ValueResult<'i, T> {
    let location = input.current_source_location();
    let name = input.expect_ident()?.clone();
    table
        .iter()
        .find(|(n, _)| name.eq_ignore_ascii_case(n))
        .map(|(_, v)| *v)
        .ok_or_else(|| location.new_custom_error(()))
}

/// One declaration's value. `Ok(None)`: not a property this renderer has.
fn parse_declaration<'i>(
    name: &str,
    input: &mut CssParser<'i, '_>,
) -> ValueResult<'i, Option<Declaration>> {
    let declaration = match name {
        "color" => Declaration::Color(parse_color(input)?),
        "background" | "background-color" => Declaration::Background(parse_color(input)?),
        "font-size" => Declaration::FontSize(parse_length(input)?),
        "font-weight" => {
            let bold = input.try_parse(|i| {
                let w = i.expect_number()?;
                Ok::<_, ParseError<'i, ()>>(w >= 600.0)
            });
            Declaration::Bold(match bold {
                Ok(b) => b,
                Err(_) => parse_keyword(input, &[("bold", true), ("normal", false)])?,
            })
        }
        "padding" => Declaration::Padding(parse_sides(input)?),
        "margin" => Declaration::Margin(parse_sides(input)?),
        "border" => parse_border(input)?,
        "border-width" => Declaration::Border(Some(parse_length(input)?), None),
        "border-color" => Declaration::Border(None, Some(parse_color(input)?)),
        "border-radius" => Declaration::BorderRadius(parse_length(input)?),
        "width" => Declaration::Width(parse_length(input)?),
        "height" => Declaration::Height(parse_length(input)?),
        "max-width" => {
            let none = input.try_parse(|i| i.expect_ident_matching("none"));
            Declaration::MaxWidth(if none.is_ok() {
                None
            } else {
                Some(parse_length(input)?)
            })
        }
        "text-align" => Declaration::TextAlign(parse_keyword(
            input,
            &[
                ("left", TextAlign::Left),
                ("start", TextAlign::Left),
                ("center", TextAlign::Center),
                ("right", TextAlign::Right),
                ("end", TextAlign::Right),
            ],
        )?),
        // No `none`: a row the keys can land on must be a row the eye can see.
        "display" => Declaration::Display(parse_keyword(
            input,
            &[
                ("block", Display::Block),
                ("grid", Display::Grid),
                ("flex", Display::Flex),
            ],
        )?),
        "grid-template-columns" => Declaration::GridColumns(parse_tracks(input)?),
        "grid-template-rows" => Declaration::GridRows(parse_tracks(input)?),
        "gap" => Declaration::Gap(parse_length(input)?),
        // Only a span: a positioned item would draw out of source order.
        "grid-column" => {
            input.expect_ident_matching("span")?;
            Declaration::ColumnSpan(input.expect_integer()?.clamp(1, 256) as u16)
        }
        "object-fit" => Declaration::ObjectFit(parse_keyword(
            input,
            &[("contain", ObjectFit::Contain), ("cover", ObjectFit::Cover)],
        )?),
        "aspect-ratio" => {
            let width = input.expect_number()?;
            let height = input
                .try_parse(|i| {
                    i.expect_delim('/')?;
                    i.expect_number()
                })
                .unwrap_or(1.0);
            if width <= 0.0 || height <= 0.0 {
                return Err(input.new_custom_error(()));
            }
            Declaration::AspectRatio(width / height)
        }
        // No reversed direction, for the same reason as above.
        "flex-wrap" => {
            Declaration::FlexWrap(parse_keyword(input, &[("wrap", true), ("nowrap", false)])?)
        }
        "justify-content" => Declaration::Justify(parse_keyword(
            input,
            &[
                ("start", Justify::Start),
                ("flex-start", Justify::Start),
                ("center", Justify::Center),
                ("end", Justify::End),
                ("flex-end", Justify::End),
                ("space-between", Justify::SpaceBetween),
            ],
        )?),
        _ => return Ok(None),
    };
    Ok(Some(declaration))
}

/// Say once per name that a stylesheet asked for something ignored, so an author
/// can find out why a rule does nothing without the log filling up every frame.
fn warn_once(what: &str) {
    static SEEN: Mutex<Option<HashSet<String>>> = Mutex::new(None);
    if let Ok(mut seen) = SEEN.lock()
        && seen
            .get_or_insert_with(HashSet::new)
            .insert(what.to_owned())
    {
        tracing::warn!("stylesheet: ignored {what}");
    }
}

// ---------------------------------------------------------------------------
// Stylesheets
// ---------------------------------------------------------------------------

#[derive(Debug, Clone)]
struct Rule {
    selectors: SelectorList<Css>,
    declarations: Vec<Declaration>,
    /// From inside `@media (prefers-color-scheme: ...)`.
    scheme: Option<Scheme>,
}

/// A parsed stylesheet. Parsing never fails: as in a browser, what cannot be read is
/// skipped and the rest still applies.
#[derive(Debug, Clone, Default)]
pub struct Stylesheet {
    rules: Vec<Rule>,
}

fn parse_body<'i>(input: &mut CssParser<'i, '_>) -> Vec<Declaration> {
    let mut declarations = Vec::new();
    while !input.is_exhausted() {
        let result: Result<(), ParseError<'i, ()>> =
            input.parse_until_after(cssparser::Delimiter::Semicolon, |decl| {
                let name = decl.expect_ident()?.to_ascii_lowercase();
                decl.expect_colon()?;
                let value = decl.parse_entirely(|value| parse_declaration(&name, value));
                match value {
                    Ok(Some(d)) => declarations.push(d),
                    Ok(None) => warn_once(&format!("property {name}")),
                    Err(_) => warn_once(&format!("a value for {name}")),
                }
                Ok(())
            });
        // A broken declaration was already skipped up to its semicolon.
        let _ = result;
    }
    declarations
}

fn media_scheme<'i>(input: &mut CssParser<'i, '_>) -> ValueResult<'i, Scheme> {
    input.expect_parenthesis_block()?;
    input.parse_nested_block(|query| {
        query.expect_ident_matching("prefers-color-scheme")?;
        query.expect_colon()?;
        parse_keyword(query, &[("dark", Scheme::Dark), ("light", Scheme::Light)])
    })
}

fn parse_rules<'i>(input: &mut CssParser<'i, '_>, scheme: Option<Scheme>, rules: &mut Vec<Rule>) {
    while !input.is_exhausted() {
        // An at-rule.
        let at: Result<String, ParseError<'i, ()>> = input.try_parse(|i| {
            let location = i.current_source_location();
            match i.next()? {
                Token::AtKeyword(name) => Ok(name.to_ascii_lowercase()),
                _ => Err(location.new_custom_error(())),
            }
        });
        if let Ok(name) = at {
            let end = cssparser::Delimiter::CurlyBracketBlock | cssparser::Delimiter::Semicolon;
            let query: Result<Scheme, ParseError<'i, ()>> = input.parse_until_before(end, |q| {
                let parsed = if name == "media" {
                    media_scheme(q)
                } else {
                    Err(q.new_custom_error(()))
                };
                while q.next().is_ok() {}
                parsed
            });
            // `@import x;` has no block and takes only itself out.
            if input.try_parse(|i| i.expect_semicolon()).is_ok() {
                warn_once(&format!("at-rule @{name}"));
                continue;
            }
            if input.expect_curly_bracket_block().is_err() {
                return;
            }
            let _: Result<(), ParseError<'i, ()>> = input.parse_nested_block(|body| {
                match query {
                    Ok(media) => parse_rules(body, Some(media), rules),
                    Err(_) => {
                        warn_once(&format!("at-rule @{name}"));
                        while body.next().is_ok() {}
                    }
                }
                Ok(())
            });
            continue;
        }
        // A rule.
        let start = input.position();
        let found: Result<(), ParseError<'i, ()>> =
            input.parse_until_before(cssparser::Delimiter::CurlyBracketBlock, |p| {
                while p.next().is_ok() {}
                Ok(())
            });
        let prelude = input.slice_from(start).to_owned();
        if found.is_err() || input.expect_curly_bracket_block().is_err() {
            return;
        }
        let declarations = input
            .parse_nested_block(|body| Ok::<_, ParseError<'i, ()>>(parse_body(body)))
            .unwrap_or_default();
        // See `MARKER_TAG`.
        let prelude = prelude.replace("::marker", &format!(" > {MARKER_TAG}"));
        let mut selector_input = ParserInput::new(&prelude);
        let mut selector_parser = CssParser::new(&mut selector_input);
        match SelectorList::parse(&SelectorParser, &mut selector_parser, ParseRelative::No) {
            Ok(selectors) => rules.push(Rule {
                selectors,
                declarations,
                scheme,
            }),
            Err(_) => warn_once(&format!("selector {}", prelude.trim())),
        }
    }
}

impl Stylesheet {
    pub fn parse(css: &str) -> Stylesheet {
        let mut input = ParserInput::new(css);
        let mut parser = CssParser::new(&mut input);
        let mut rules = Vec::new();
        parse_rules(&mut parser, None, &mut rules);
        Stylesheet { rules }
    }

    pub fn is_empty(&self) -> bool {
        self.rules.is_empty()
    }
}

// ---------------------------------------------------------------------------
// The cascade
// ---------------------------------------------------------------------------

/// What computing a style depends on outside the stylesheet.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct StyleEnv {
    /// The app's font size, in pixels.
    pub base_em: f32,
    /// The width of a character at that size, in pixels.
    pub base_ch: f32,
    /// Pixels per CSS px.
    pub px_scale: f32,
    pub scheme: Scheme,
}

impl Default for StyleEnv {
    fn default() -> Self {
        StyleEnv {
            base_em: 16.0,
            base_ch: 10.0,
            px_scale: 1.0,
            scheme: Scheme::Dark,
        }
    }
}

fn matches(
    selector: &Selector<Css>,
    element: &PathElement<'_>,
    caches: &mut SelectorCaches,
) -> bool {
    let mut cx = MatchingContext::new(
        MatchingMode::Normal,
        None,
        caches,
        QuirksMode::NoQuirks,
        NeedsSelectorFlags::No,
        MatchingForInvalidation::No,
    );
    matching::matches_selector(selector, 0, None, element, &mut cx)
}

/// Apply `sheets`, in order, to one element. `path` runs from the `ul` to it.
fn style_of(sheets: &[&Stylesheet], path: &[StyleNode], parent: &Style, env: &StyleEnv) -> Style {
    let element = PathElement {
        path,
        index: path.len() - 1,
    };
    let mut caches = SelectorCaches::default();
    let mut style = parent.inherited();
    for sheet in sheets {
        // Specificity within a sheet, then its source order: a stable sort keeps it.
        let mut matched: Vec<(u32, &Rule)> = sheet
            .rules
            .iter()
            .filter(|rule| rule.scheme.is_none_or(|s| s == env.scheme))
            .filter_map(|rule| {
                rule.selectors
                    .slice()
                    .iter()
                    .filter(|selector| matches(selector, &element, &mut caches))
                    .map(|selector| selector.specificity())
                    .max()
                    .map(|specificity| (specificity, rule))
            })
            .collect();
        matched.sort_by_key(|(specificity, _)| *specificity);
        for (_, rule) in matched {
            for declaration in &rule.declarations {
                declaration.apply(&mut style, env, parent.font_scale);
            }
        }
    }
    style
}

/// Compute an element's style, walking down `path` so inheritance is real.
pub fn compute(sheets: &[&Stylesheet], path: &[StyleNode], env: &StyleEnv) -> Style {
    let mut style = Style::default();
    for depth in 1..=path.len() {
        style = style_of(sheets, &path[..depth], &style, env);
    }
    style
}

/// A `<style>` row's sheet. It applies to the list it was in and everything below,
/// for as long as the element that list belongs to is still there: `root` is that
/// element's position in the tree and `key` its text, so a sheet cannot end up on
/// whatever moved into its place.
#[derive(Debug, Clone)]
struct ScopedSheet {
    root: IdArray,
    key: String,
    css: String,
    sheet: Arc<Stylesheet>,
}

/// The stylesheets in force, in cascade order.
#[derive(Debug, Clone)]
pub struct Sheets {
    default: Stylesheet,
    user: Option<Stylesheet>,
    scoped: Vec<ScopedSheet>,
    /// FFON documents a `<link>` brought in, whose rows are markup whatever the
    /// provider around them says: (the row holding the document, its text).
    documents: Vec<(IdArray, String)>,
    /// The sheets that apply to the current list.
    active: Vec<Arc<Stylesheet>>,
    /// The current list's own classes: those of the row it belongs to.
    ul_classes: Vec<String>,
    /// The current list's rows are markup.
    markup_here: bool,
    /// Computed row styles, by the row's text and whether the cursor is on it. Rows
    /// that look alike share one, so a long list costs a few lookups a frame.
    cache: RefCell<HashMap<RowKey, Arc<RowStyle>>>,
    /// Rows of boxes already laid out, see `boxes::layout_row`.
    pub(crate) box_cache: RefCell<HashMap<crate::boxes::BoxKey, Arc<crate::boxes::RowLayout>>>,
}

impl Default for Sheets {
    fn default() -> Self {
        Sheets {
            default: Stylesheet::parse(DEFAULT_CSS),
            user: None,
            scoped: Vec::new(),
            documents: Vec::new(),
            active: Vec::new(),
            ul_classes: Vec::new(),
            markup_here: false,
            cache: RefCell::new(HashMap::new()),
            box_cache: RefCell::new(HashMap::new()),
        }
    }
}

/// What a computed row style depends on: the row, whether it is markup, whether the
/// cursor is on it, and the colour scheme.
type RowKey = (String, bool, bool, Scheme);

/// `root` is `id` or above it in the tree.
pub(crate) fn id_covers(root: &IdArray, id: &IdArray) -> bool {
    root.depth() <= id.depth() && (0..root.depth()).all(|d| root.get(d) == id.get(d))
}

/// A `Style` for one row from its `li` and the element the whole row is: what the
/// element says, and what the `li` says about anything the element leaves alone.
fn merge_row_box(li: &Style, element: Style) -> Style {
    let plain = Style::default();
    Style {
        background: element.background.or(li.background),
        padding: if element.padding == plain.padding {
            li.padding
        } else {
            element.padding
        },
        margin: if element.margin == plain.margin {
            li.margin
        } else {
            element.margin
        },
        border_width: if element.border_width == plain.border_width {
            li.border_width
        } else {
            element.border_width
        },
        border_color: element.border_color.or(li.border_color),
        border_radius: if element.border_radius == plain.border_radius {
            li.border_radius
        } else {
            element.border_radius
        },
        ..element
    }
}

fn node_of(element: &markup::Element) -> StyleNode {
    StyleNode {
        tag: element.tag.to_owned(),
        classes: element.classes.clone(),
        focus: false,
    }
}

/// Links and buttons stand out wherever a row has them, before anything is pressed:
/// their text takes what `a` and `button` say, which by default is bold and a colour.
/// Whatever an element of the row already styled is left alone.
fn target_runs(
    sheets: &[&Stylesheet],
    row: &str,
    markup: bool,
    path: &[StyleNode],
    env: &StyleEnv,
    runs: &mut Vec<TextRun>,
) {
    let found = if markup {
        markup::targets(row)
    } else {
        tags::targets(row)
    };
    for target in found {
        let tag = match target.kind {
            tags::TargetKind::Link => "link",
            tags::TargetKind::Button => "button",
            _ => continue,
        };
        let mut path = path.to_vec();
        path.push(StyleNode::new(tag));
        let style = compute(sheets, &path, env);
        let (start, end) = (target.display_span.start, target.display_span.end);
        let taken = runs.iter().any(|r| r.start < end && start < r.end);
        let nothing = style.color.is_none() && !style.bold && style.background.is_none();
        if nothing || end <= start || taken {
            continue;
        }
        runs.push(TextRun {
            start,
            end,
            color: style.color,
            bold: style.bold,
            background: style.background,
        });
    }
    runs.sort_by_key(|r| r.start);
}

/// Where inline elements colour or embolden the row's text. `at` counts the
/// characters of what the row shows.
fn collect_runs(
    sheets: &[&Stylesheet],
    nodes: &[markup::Node],
    path: &mut Vec<StyleNode>,
    inherited: Option<(Option<u32>, bool)>,
    env: &StyleEnv,
    at: &mut usize,
    runs: &mut Vec<TextRun>,
) {
    for node in nodes {
        match node {
            markup::Node::Text(text) => {
                let len = tags::strip_display(text).chars().count();
                if let Some((color, bold)) = inherited
                    && len > 0
                {
                    runs.push(TextRun {
                        start: *at,
                        end: *at + len,
                        color,
                        bold,
                        background: None,
                    });
                }
                *at += len;
            }
            // These three are drawn from what they stand for, so they take up what
            // `markup::plain` writes for them, not their children.
            markup::Node::Element(element) if matches!(element.tag, "img" | "a" | "video") => {
                path.push(node_of(element));
                let style = compute(sheets, path, env);
                path.pop();
                let shown = markup::plain_nodes(std::slice::from_ref(node));
                let len = tags::strip_display(&shown).chars().count();
                if element.tag == "a"
                    && (style.color.is_some() || style.bold || style.background.is_some())
                    && len > 0
                {
                    runs.push(TextRun {
                        start: *at,
                        end: *at + len,
                        color: style.color,
                        bold: style.bold,
                        background: style.background,
                    });
                }
                *at += len;
            }
            markup::Node::Element(element) => {
                path.push(node_of(element));
                let style = compute(sheets, path, env);
                collect_runs(
                    sheets,
                    &element.children,
                    path,
                    Some((style.color, style.bold)),
                    env,
                    at,
                    runs,
                );
                path.pop();
            }
        }
    }
}

impl Sheets {
    /// The user's `user.css`, which comes last so it wins.
    pub fn set_user(&mut self, css: Option<&str>) {
        self.user = css.map(Stylesheet::parse).filter(|s| !s.is_empty());
        self.clear_caches();
    }

    fn clear_caches(&self) {
        self.cache.borrow_mut().clear();
        self.box_cache.borrow_mut().clear();
    }

    /// Remember the sheet of a `<style>` row found in the list under `root`.
    pub(crate) fn set_scoped(&mut self, root: &IdArray, key: &str, css: String) {
        if let Some(found) = self
            .scoped
            .iter_mut()
            .find(|s| s.root == *root && s.key == key)
        {
            if found.css != css {
                found.sheet = Arc::new(Stylesheet::parse(&css));
                found.css = css;
                self.clear_caches();
            }
            return;
        }
        self.scoped.push(ScopedSheet {
            root: root.clone(),
            key: key.to_owned(),
            sheet: Arc::new(Stylesheet::parse(&css)),
            css,
        });
    }

    /// Say that the row at `root`, with this text, holds an FFON document.
    pub(crate) fn mark_document(&mut self, root: &IdArray, key: &str) {
        if !self.documents.iter().any(|(r, k)| r == root && k == key) {
            self.documents.push((root.clone(), key.to_owned()));
        }
    }

    /// The documents still in the tree, by `key_at`. Forgets the others.
    pub(crate) fn live_documents(
        &mut self,
        key_at: impl Fn(&IdArray) -> Option<String>,
    ) -> Vec<IdArray> {
        self.documents
            .retain(|(root, key)| key_at(root).as_deref() == Some(key.as_str()));
        self.documents
            .iter()
            .map(|(root, _)| root.clone())
            .collect()
    }

    /// Work out what applies to the list under `parent`.
    pub(crate) fn enter_list(
        &mut self,
        parent: &IdArray,
        markup_here: bool,
        ul_classes: Vec<String>,
        key_at: impl Fn(&IdArray) -> Option<String>,
    ) {
        self.scoped
            .retain(|s| key_at(&s.root).as_deref() == Some(s.key.as_str()));
        let active: Vec<Arc<Stylesheet>> = self
            .scoped
            .iter()
            .filter(|s| id_covers(&s.root, parent))
            .map(|s| Arc::clone(&s.sheet))
            .collect();
        let changed = active.len() != self.active.len()
            || active
                .iter()
                .zip(&self.active)
                .any(|(a, b)| !Arc::ptr_eq(a, b))
            || ul_classes != self.ul_classes;
        if changed {
            self.clear_caches();
        }
        self.active = active;
        self.ul_classes = ul_classes;
        self.markup_here = markup_here;
    }

    /// Whether the current list's rows are markup.
    pub fn markup_here(&self) -> bool {
        self.markup_here
    }

    fn all(&self) -> Vec<&Stylesheet> {
        std::iter::once(&self.default)
            .chain(self.active.iter().map(Arc::as_ref))
            .chain(&self.user)
            .collect()
    }

    fn ul_node(&self) -> StyleNode {
        StyleNode {
            classes: self.ul_classes.clone(),
            ..StyleNode::new("ul")
        }
    }

    /// The style of the element at the end of `path`, which starts at the `ul`.
    pub(crate) fn style_for(&self, path: &[StyleNode], env: &StyleEnv) -> Style {
        compute(&self.all(), path, env)
    }

    /// Where inline elements colour or embolden the text of `nodes`, which sit under the
    /// element at the end of `parent_path`.
    pub(crate) fn runs_for(
        &self,
        parent_path: &[StyleNode],
        nodes: &[markup::Node],
        env: &StyleEnv,
    ) -> Vec<TextRun> {
        let mut path = parent_path.to_vec();
        let mut runs = Vec::new();
        collect_runs(&self.all(), nodes, &mut path, None, env, &mut 0, &mut runs);
        runs
    }

    /// The `li` node of a row, which the box tree of a row hangs from.
    pub(crate) fn row_path(&self, focus: bool) -> Vec<StyleNode> {
        vec![
            self.ul_node(),
            StyleNode {
                focus,
                ..StyleNode::new("li")
            },
        ]
    }

    /// The `ul` of the current list.
    pub fn list_style(&self, env: &StyleEnv) -> Style {
        compute(&self.all(), &[self.ul_node()], env)
    }

    /// A row of the current list. `row` is its text if it holds markup.
    pub fn row_style(&self, row: Option<&str>, focus: bool, env: &StyleEnv) -> Arc<RowStyle> {
        self.row_style_for(row, row.is_some(), focus, env)
    }

    /// [`Sheets::row_style`] for a row that is not markup but holds links or buttons, which
    /// stand out wherever they are: `markup` says how to read the row.
    pub fn row_style_for(
        &self,
        row: Option<&str>,
        markup: bool,
        focus: bool,
        env: &StyleEnv,
    ) -> Arc<RowStyle> {
        // The colour scheme is part of it: `@media (prefers-color-scheme)` is decided here, and
        // the scheme can change while the app runs.
        let key = (
            row.unwrap_or_default().to_owned(),
            markup,
            focus,
            env.scheme,
        );
        if let Some(found) = self.cache.borrow().get(&key) {
            return Arc::clone(found);
        }
        let style = Arc::new(self.compute_row(row, markup, focus, env));
        let mut cache = self.cache.borrow_mut();
        // Rows whose text keeps changing must not grow this for ever.
        if cache.len() > 4096 {
            cache.clear();
        }
        cache.insert(key, Arc::clone(&style));
        style
    }

    fn compute_row(
        &self,
        row: Option<&str>,
        markup: bool,
        focus: bool,
        env: &StyleEnv,
    ) -> RowStyle {
        let row = row.map(crate::list::without_toggle_tags);
        let row = row.as_deref();
        let sheets = self.all();
        let li = |focus| StyleNode {
            focus,
            ..StyleNode::new("li")
        };
        let ul = self.ul_node();
        let nodes = row
            .filter(|_| markup)
            .map(markup::parse)
            .unwrap_or_default();
        let element = markup::whole_row_element(&nodes);
        let mut path = vec![ul.clone(), li(focus)];
        let mut style = compute(&sheets, &path, env);
        if let Some(element) = element {
            path.push(node_of(element));
            style = merge_row_box(&style, compute(&sheets, &path, env));
            path.pop();
        }
        let mut runs = Vec::new();
        let mut at = 0;
        let inner = element.map_or(nodes.as_slice(), |e| e.children.as_slice());
        if let Some(element) = element {
            path.push(node_of(element));
        }
        collect_runs(&sheets, inner, &mut path, None, env, &mut at, &mut runs);
        if let Some(row) = row {
            target_runs(
                &sheets,
                row,
                markup,
                &[ul.clone(), li(focus)],
                env,
                &mut runs,
            );
        }
        let marker = StyleNode::new(MARKER_TAG);
        let boxed = markup
            && row.is_some()
            && crate::boxes::needs_boxes(&nodes, self, env, &[ul.clone(), li(focus)]);
        RowStyle {
            boxed,
            li: style,
            marker_color: compute(&sheets, &[ul.clone(), li(focus), marker], env).color,
            unfocused_background: focus
                .then(|| compute(&sheets, &[ul, li(false)], env).background)
                .flatten(),
            runs,
        }
    }
}

// ---------------------------------------------------------------------------
// Guards
// ---------------------------------------------------------------------------

fn linear(channel: u32) -> f64 {
    let v = (channel & 0xFF) as f64 / 255.0;
    if v <= 0.03928 {
        v / 12.92
    } else {
        ((v + 0.055) / 1.055).powf(2.4)
    }
}

fn luminance(rgba: u32) -> f64 {
    0.2126 * linear(rgba >> 24) + 0.7152 * linear(rgba >> 16) + 0.0722 * linear(rgba >> 8)
}

/// WCAG contrast ratio of two opaque colours, from 1 to 21.
pub fn contrast(a: u32, b: u32) -> f64 {
    let (la, lb) = (luminance(a), luminance(b));
    (la.max(lb) + 0.05) / (la.min(lb) + 0.05)
}

/// WCAG AA for ordinary text.
pub const MIN_CONTRAST: f64 = 4.5;

/// A stylesheet's text colour on its background, or `fallback` where it could not be
/// read: no stylesheet gets to make text unreadable.
pub fn readable_color(wanted: Option<u32>, background: u32, fallback: u32) -> u32 {
    match wanted {
        Some(color) if contrast(color, background) >= MIN_CONTRAST => color,
        _ => fallback,
    }
}

/// The colour that marks the row the cursor is on, or `fallback` where it would not show
/// against the list's own: no stylesheet gets to hide the cursor.
pub fn visible_focus(wanted: Option<u32>, list_background: u32, fallback: u32) -> u32 {
    const MIN_FOCUS_CONTRAST: f64 = 1.2;
    match wanted {
        Some(color)
            if color & 0xFF == 0xFF && contrast(color, list_background) >= MIN_FOCUS_CONTRAST =>
        {
            color
        }
        _ => fallback,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn node(tag: &str, classes: &[&str]) -> StyleNode {
        StyleNode {
            tag: tag.into(),
            classes: classes.iter().map(|c| c.to_string()).collect(),
            focus: false,
        }
    }

    fn focused(tag: &str) -> StyleNode {
        StyleNode {
            focus: true,
            ..node(tag, &[])
        }
    }

    fn style(css: &str, path: &[StyleNode]) -> Style {
        compute(&[&Stylesheet::parse(css)], path, &StyleEnv::default())
    }

    fn li(css: &str) -> Style {
        style(css, &[node("ul", &[]), node("li", &[])])
    }

    /// Links and buttons stand out before `i` or `a`, in a row of plain text and in one of
    /// markup, and a sheet can restyle them.
    #[test]
    fn links_and_buttons_stand_out_in_every_row() {
        let env = StyleEnv::default();
        let sheets = Sheets::default();
        let runs = |row: &str, markup: bool| {
            sheets
                .row_style_for(Some(row), markup, false, &env)
                .runs
                .clone()
        };

        let row = "First <link>a</link> then <link>b</link>";
        let found = runs(row, false);
        assert_eq!(found.len(), 2, "{found:?}");
        assert!(
            found.iter().all(|r| r.bold && r.color == Some(0x267AFFFF)),
            "{found:?}"
        );
        assert_eq!(
            (found[0].start, found[0].end),
            (0, 7),
            "\"First a\", as shown"
        );

        let found = runs("<button>f</button>Run it", false);
        assert_eq!(found.len(), 1);
        assert!(
            found[0].bold && found[0].color == Some(0xFFFFFFFF),
            "{found:?}"
        );
        assert_eq!(
            found[0].background,
            Some(0x264F78FF),
            "a button has a pill behind it"
        );
        assert!(runs("First <link>a</link>", false)[0].background.is_none());

        assert_eq!(runs(r#"<p>See <a href="u">here</a></p>"#, true).len(), 1);
        assert!(runs("just text", false).is_empty());
        assert!(runs("a picture <image>p.png</image>", false).is_empty());

        // Both colour schemes read well: every colour has 4.5:1 against the plain
        // background of its scheme, and against the pill behind it.
        for (scheme, background) in [(Scheme::Dark, 0x000000FF), (Scheme::Light, 0xFFFFFFFF)] {
            let env = StyleEnv {
                scheme,
                ..StyleEnv::default()
            };
            let link = sheets.row_style_for(Some(row), false, false, &env).runs[0];
            assert!(
                contrast(link.color.unwrap(), background) >= MIN_CONTRAST,
                "{scheme:?} link"
            );
            let button = sheets
                .row_style_for(Some("<button>f</button>Go"), false, false, &env)
                .runs[0];
            assert!(
                contrast(button.color.unwrap(), button.background.unwrap()) >= MIN_CONTRAST,
                "{scheme:?} button"
            );
            assert!(
                button.background.unwrap() != background,
                "{scheme:?} the pill is not the list's own background"
            );
        }

        let mut own = Sheets::default();
        own.set_user(Some(
            "a { color: #FF0000 } button { font-weight: normal; color: #00FF00 }",
        ));
        let link = own.row_style_for(Some(row), false, false, &env).runs[0];
        assert_eq!(link.color, Some(0xFF0000FF));
        assert!(
            own.row_style_for(Some("<button>f</button>Go"), false, false, &env)
                .runs[0]
                .color
                == Some(0x00FF00FF)
        );
    }

    #[test]
    fn the_default_sheet_keeps_every_list_at_120_characters() {
        let ul = style(DEFAULT_CSS, &[node("ul", &[])]);
        assert_eq!(ul.max_width, Some(Length::Ch(120.0)));
    }

    #[test]
    fn a_later_sheet_can_go_full_width() {
        let default = Stylesheet::parse(DEFAULT_CSS);
        let site = Stylesheet::parse("ul { max-width: none }");
        let ul = compute(&[&default, &site], &[node("ul", &[])], &StyleEnv::default());
        assert_eq!(ul.max_width, None);
    }

    #[test]
    fn tag_class_and_descendant_selectors_match() {
        let path = [node("ul", &["album"]), node("li", &["done"])];
        assert_eq!(style("li { color: #fff }", &path).color, Some(0xFFFFFFFF));
        assert_eq!(
            style(".done { color: #fff }", &path).color,
            Some(0xFFFFFFFF)
        );
        assert_eq!(
            style("ul.album li { color: #fff }", &path).color,
            Some(0xFFFFFFFF)
        );
        assert_eq!(style("ul.other li { color: #fff }", &path).color, None);
        assert_eq!(style("p { color: #fff }", &path).color, None);
    }

    #[test]
    fn focus_matches_only_the_row_the_cursor_is_on() {
        let css = "li:focus { background: #234 }";
        assert_eq!(li(css).background, None);
        let on = style(css, &[node("ul", &[]), focused("li")]);
        assert_eq!(on.background, Some(0x223344FF));
    }

    #[test]
    fn marker_styles_the_list_prefix_and_not_the_row() {
        let css = "li::marker { color: #e80 }";
        assert_eq!(li(css).color, None);
        let marker = style(
            css,
            &[node("ul", &[]), node("li", &[]), node(MARKER_TAG, &[])],
        );
        assert_eq!(marker.color, Some(0xEE8800FF));
    }

    #[test]
    fn marker_can_follow_the_focus() {
        let css = "li:focus::marker { color: #e80 }";
        let off = [node("ul", &[]), node("li", &[]), node(MARKER_TAG, &[])];
        let on = [node("ul", &[]), focused("li"), node(MARKER_TAG, &[])];
        assert_eq!(style(css, &off).color, None);
        assert_eq!(style(css, &on).color, Some(0xEE8800FF));
    }

    /// Written the web's way, these still find our tags.
    #[test]
    fn img_and_a_find_images_and_links() {
        let image = [node("ul", &[]), node("image", &[])];
        let link = [node("ul", &[]), node("link", &[])];
        assert_eq!(style("img { width: 10px }", &image).width, Length::Px(10.0));
        assert_eq!(style("a { color: #fff }", &link).color, Some(0xFFFFFFFF));
    }

    #[test]
    fn specificity_beats_order_and_order_breaks_ties() {
        let path = [node("ul", &[]), node("li", &["x"])];
        let css = ".x { color: #111 } li { color: #222 }";
        assert_eq!(style(css, &path).color, Some(0x111111FF));
        let css = "li { color: #111 } li { color: #222 }";
        assert_eq!(style(css, &path).color, Some(0x222222FF));
    }

    /// `user.css` comes last, so it wins whatever a site's specificity is.
    #[test]
    fn a_later_sheet_beats_a_more_specific_earlier_one() {
        let site = Stylesheet::parse("ul li.x { color: #111 }");
        let user = Stylesheet::parse("li { color: #222 }");
        let path = [node("ul", &[]), node("li", &["x"])];
        let got = compute(&[&site, &user], &path, &StyleEnv::default());
        assert_eq!(got.color, Some(0x222222FF));
    }

    #[test]
    fn color_inherits_and_background_does_not() {
        let css = "ul { color: #111; background: #222 }";
        let got = li(css);
        assert_eq!(got.color, Some(0x111111FF));
        assert_eq!(got.background, None);
    }

    #[test]
    fn font_size_is_relative_to_the_parent() {
        let env = StyleEnv {
            base_em: 20.0,
            ..Default::default()
        };
        let sheet = Stylesheet::parse("ul { font-size: 2em } li { font-size: 150% }");
        let path = [node("ul", &[]), node("li", &[])];
        assert_eq!(compute(&[&sheet], &path, &env).font_scale, 3.0);
    }

    /// A stylesheet's 24px grows with the font-scale setting like everything else.
    #[test]
    fn px_follows_the_font_scale_setting() {
        let env = StyleEnv {
            base_em: 20.0,
            px_scale: 2.0,
            ..Default::default()
        };
        let sheet = Stylesheet::parse("li { font-size: 20px }");
        let path = [node("ul", &[]), node("li", &[])];
        assert_eq!(compute(&[&sheet], &path, &env).font_scale, 2.0);
    }

    #[test]
    fn the_color_forms_all_read() {
        for (css, want) in [
            ("#abc", 0xAABBCCFF),
            ("#abcd", 0xAABBCCDD),
            ("#a1b2c3", 0xA1B2C3FF),
            ("#a1b2c3d4", 0xA1B2C3D4),
            ("white", 0xFFFFFFFF),
            ("WHITE", 0xFFFFFFFF),
            ("rgb(1, 2, 3)", 0x010203FF),
            ("rgb(1 2 3)", 0x010203FF),
            ("rgba(1, 2, 3, 0.5)", 0x01020380),
            ("rgb(1 2 3 / 0.5)", 0x01020380),
        ] {
            assert_eq!(
                li(&format!("li {{ color: {css} }}")).color,
                Some(want),
                "{css}"
            );
        }
    }

    #[test]
    fn sides_read_the_way_css_writes_them() {
        let (a, b, c, d) = (
            Length::Px(1.0),
            Length::Px(2.0),
            Length::Px(3.0),
            Length::Px(4.0),
        );
        assert_eq!(li("li { padding: 1px }").padding, [a, a, a, a]);
        assert_eq!(li("li { padding: 1px 2px }").padding, [a, b, a, b]);
        assert_eq!(li("li { padding: 1px 2px 3px }").padding, [a, b, c, b]);
        assert_eq!(li("li { margin: 1px 2px 3px 4px }").margin, [a, b, c, d]);
    }

    #[test]
    fn border_reads_width_style_and_color_in_any_order() {
        let got = li("li { border: 1px solid #444 }");
        assert_eq!(got.border_width, Length::Px(1.0));
        assert_eq!(got.border_color, Some(0x444444FF));
        let got = li("li { border: #444 solid 2px; border-radius: 5px }");
        assert_eq!(got.border_width, Length::Px(2.0));
        assert_eq!(got.border_radius, Length::Px(5.0));
    }

    /// The user's example, every declaration of it.
    #[test]
    fn the_grid_example_reads() {
        let css = ".grid-container {
            /* Turn the element into a grid container */
            display: grid;
            grid-template-columns: repeat(3, 1fr);
            grid-template-rows: auto;
            gap: 15px;
        }
        .grid-item {
            background-color: #3498db;
            color: white;
            padding: 20px;
            text-align: center;
        }";
        let container = node("div", &["grid-container"]);
        let got = style(css, std::slice::from_ref(&container));
        assert_eq!(got.display, Display::Grid);
        assert_eq!(got.grid_columns, vec![Length::Fr(1.0); 3]);
        assert_eq!(got.grid_rows, vec![Length::Auto]);
        assert_eq!(got.gap, Length::Px(15.0));
        let item = style(css, &[container, node("div", &["grid-item"])]);
        assert_eq!(item.background, Some(0x3498DBFF));
        assert_eq!(item.color, Some(0xFFFFFFFF));
        assert_eq!(item.padding, [Length::Px(20.0); 4]);
        assert_eq!(item.text_align, TextAlign::Center);
    }

    #[test]
    fn tracks_mix_repeat_and_plain_ones() {
        let got = li("li { grid-template-columns: 100px repeat(2, 1fr 2fr) auto }");
        assert_eq!(
            got.grid_columns,
            vec![
                Length::Px(100.0),
                Length::Fr(1.0),
                Length::Fr(2.0),
                Length::Fr(1.0),
                Length::Fr(2.0),
                Length::Auto,
            ]
        );
    }

    #[test]
    fn a_huge_repeat_is_bounded() {
        let got = li("li { grid-template-columns: repeat(99999999, 1fr) }");
        assert_eq!(got.grid_columns.len(), 256);
    }

    #[test]
    fn the_remaining_grid_and_flex_properties_read() {
        let got = li(
            "li { grid-column: span 2; object-fit: cover; aspect-ratio: 16 / 9;
                           display: flex; flex-wrap: wrap; justify-content: space-between }",
        );
        assert_eq!(got.column_span, 2);
        assert_eq!(got.object_fit, ObjectFit::Cover);
        assert_eq!(got.aspect_ratio, Some(16.0 / 9.0));
        assert_eq!(got.display, Display::Flex);
        assert!(got.flex_wrap);
        assert_eq!(got.justify, Justify::SpaceBetween);
    }

    /// Keys and speech follow source order, so a stylesheet cannot change it.
    #[test]
    fn what_would_break_source_order_is_ignored() {
        let plain = li("");
        for css in [
            "li { display: none }",
            "li { order: 2 }",
            "li { content: 'x' }",
            "li { flex-direction: row-reverse }",
            "li { grid-column: 2 / 3 }",
            "li { visibility: hidden }",
        ] {
            assert_eq!(li(css), plain, "{css}");
        }
    }

    /// No network from a stylesheet.
    #[test]
    fn url_values_are_ignored() {
        assert_eq!(
            li("li { background: url(http://x.org/a.png) }").background,
            None
        );
    }

    /// Like a browser: what cannot be read is skipped, the rest still applies.
    #[test]
    fn broken_css_loses_only_what_is_broken() {
        let css = "li { color: not-a-color; padding: 1px; nonsense; background: #111 }
                   !!! { color: #000 }
                   li { width: 5px }";
        let got = li(css);
        assert_eq!(got.color, None);
        assert_eq!(got.padding, [Length::Px(1.0); 4]);
        assert_eq!(got.background, Some(0x111111FF));
        assert_eq!(got.width, Length::Px(5.0));
    }

    #[test]
    fn nothing_at_all_can_be_parsed() {
        for css in [
            "", "   ", "}", "{", "li {", "li", "@media", "@media (", "/* open",
        ] {
            let _ = li(css);
        }
    }

    #[test]
    fn media_follows_the_color_scheme() {
        let sheet = Stylesheet::parse(
            "@media (prefers-color-scheme: dark) { li { color: #111 } }
             @media (prefers-color-scheme: light) { li { color: #222 } }",
        );
        let path = [node("ul", &[]), node("li", &[])];
        let dark = StyleEnv {
            scheme: Scheme::Dark,
            ..Default::default()
        };
        let light = StyleEnv {
            scheme: Scheme::Light,
            ..Default::default()
        };
        assert_eq!(compute(&[&sheet], &path, &dark).color, Some(0x111111FF));
        assert_eq!(compute(&[&sheet], &path, &light).color, Some(0x222222FF));
    }

    #[test]
    fn an_at_rule_with_no_block_does_not_swallow_the_next_rule() {
        assert_eq!(
            li("@import 'x.css'; li { width: 1px }").width,
            Length::Px(1.0)
        );
    }

    #[test]
    fn an_unknown_at_rule_takes_only_itself_out() {
        let got = li("@import 'x.css'; @supports (a: b) { li { color: #111 } } li { width: 1px }");
        assert_eq!(got.color, None);
        assert_eq!(got.width, Length::Px(1.0));
    }

    #[test]
    fn lengths_resolve_against_their_context() {
        let cx = LengthContext {
            em: 20.0,
            ch: 10.0,
            percent_of: 400.0,
            px_scale: 2.0,
        };
        assert_eq!(Length::Px(3.0).px(&cx), Some(6.0));
        assert_eq!(Length::Em(2.0).px(&cx), Some(40.0));
        assert_eq!(Length::Ch(120.0).px(&cx), Some(1200.0));
        assert_eq!(Length::Percent(50.0).px(&cx), Some(200.0));
        assert_eq!(Length::Auto.px(&cx), None);
        assert_eq!(Length::Fr(1.0).px(&cx), None);
    }

    #[test]
    fn a_user_sheet_changes_a_row_only_while_it_says_something() {
        let env = StyleEnv::default();
        let color = |sheets: &Sheets| sheets.row_style(None, false, &env).li.color;
        let mut sheets = Sheets::default();
        assert_eq!(color(&sheets), None);
        sheets.set_user(Some("  /* nothing */ "));
        assert_eq!(color(&sheets), None, "an empty file is no stylesheet");
        sheets.set_user(Some("li { color: #fff }"));
        assert_eq!(color(&sheets), Some(0xffffffff));
        sheets.set_user(None);
        assert_eq!(color(&sheets), None);
    }

    /// Every background is rounded unless a stylesheet says otherwise: a focused row,
    /// a coloured row and a box inside a row.
    #[test]
    fn every_background_is_rounded_by_default() {
        let env = StyleEnv::default();
        let radius = |sheets: &Sheets, focus| sheets.row_style(None, focus, &env).li.border_radius;
        let mut sheets = Sheets::default();
        assert_eq!(radius(&sheets, true), Length::Px(DEFAULT_RADIUS));
        assert_eq!(radius(&sheets, false), Length::Px(DEFAULT_RADIUS));
        sheets.set_user(Some("li { background: #123456 }"));
        assert_eq!(radius(&sheets, false), Length::Px(DEFAULT_RADIUS));
        sheets.set_user(Some("li { border-radius: 0 }"));
        assert_eq!(
            radius(&sheets, true),
            Length::Px(0.0),
            "a sheet can square it"
        );
    }

    #[test]
    fn the_user_sheet_overrides_the_default_width() {
        let mut sheets = Sheets::default();
        let env = StyleEnv::default();
        assert_eq!(sheets.list_style(&env).max_width, Some(Length::Ch(120.0)));
        sheets.set_user(Some("ul { max-width: none }"));
        assert_eq!(sheets.list_style(&env).max_width, None);
    }

    #[test]
    fn a_row_knows_its_marker_and_its_background_without_the_cursor() {
        let mut sheets = Sheets::default();
        let env = StyleEnv::default();
        sheets.set_user(Some(
            "li { background: #111 } li:focus { background: #222 } li::marker { color: #333 }",
        ));
        let plain = sheets.row_style(None, false, &env);
        assert_eq!(plain.li.background, Some(0x111111FF));
        assert_eq!(plain.marker_color, Some(0x333333FF));
        assert_eq!(plain.unfocused_background, None);
        let on = sheets.row_style(None, true, &env);
        assert_eq!(on.li.background, Some(0x222222FF));
        assert_eq!(on.unfocused_background, Some(0x111111FF));
    }

    fn id(path: &[usize]) -> IdArray {
        let mut id = IdArray::new();
        for &p in path {
            id.push(p);
        }
        id
    }

    fn sheets_with(css: &str) -> Sheets {
        let mut sheets = Sheets::default();
        sheets.set_user(Some(css));
        sheets
    }

    #[test]
    fn a_heading_is_big_and_bold_with_no_stylesheet_at_all() {
        let sheets = Sheets::default();
        let style = sheets.row_style(Some("<h1>Title</h1>"), false, &StyleEnv::default());
        assert_eq!(style.li.font_scale, 2.0);
        assert!(style.li.bold);
    }

    #[test]
    fn a_class_on_the_whole_row_styles_the_row() {
        let sheets = sheets_with(".done { color: #888; padding: 1px }");
        let style = sheets.row_style(
            Some(r#"<p class="done">Shipped</p>"#),
            false,
            &StyleEnv::default(),
        );
        assert_eq!(style.li.color, Some(0x888888FF));
        assert_eq!(style.li.padding, [Length::Px(1.0); 4]);
        assert!(style.runs.is_empty());
    }

    /// The row's `li` and the element inside both count: one sets the box, one the
    /// text, and the element wins where both speak.
    #[test]
    fn the_li_and_the_element_inside_it_both_count() {
        let sheets =
            sheets_with("li { background: #111; padding: 2px; color: #222 } p { color: #333 }");
        let style = sheets.row_style(Some("<p>x</p>"), false, &StyleEnv::default());
        assert_eq!(style.li.background, Some(0x111111FF));
        assert_eq!(style.li.padding, [Length::Px(2.0); 4]);
        assert_eq!(style.li.color, Some(0x333333FF));
    }

    #[test]
    fn selectors_see_the_row_element_inside_its_li() {
        let sheets = sheets_with("li h1 { color: #111 } ul > li > p { color: #222 }");
        let env = StyleEnv::default();
        assert_eq!(
            sheets.row_style(Some("<h1>x</h1>"), false, &env).li.color,
            Some(0x111111FF)
        );
        assert_eq!(
            sheets.row_style(Some("<p>x</p>"), false, &env).li.color,
            Some(0x222222FF)
        );
    }

    #[test]
    fn inline_elements_become_runs_at_their_characters() {
        let sheets = sheets_with(".hot { color: #e80 }");
        let row = r#"one <b>two</b> <span class="hot">three</span>"#;
        let style = sheets.row_style(Some(row), false, &StyleEnv::default());
        assert_eq!(style.runs.len(), 2);
        assert_eq!(
            (style.runs[0].start, style.runs[0].end, style.runs[0].bold),
            (4, 7, true)
        );
        assert_eq!((style.runs[1].start, style.runs[1].end), (8, 13));
        assert_eq!(style.runs[1].color, Some(0xEE8800FF));
    }

    /// The older tags inside take up what they show, which is what the row's label is.
    #[test]
    fn runs_count_what_is_shown_and_not_the_tags() {
        let sheets = Sheets::default();
        let row = "<button>f</button>Go <b>now</b>";
        let style = sheets.row_style(Some(row), false, &StyleEnv::default());
        assert_eq!((style.runs[0].start, style.runs[0].end), (3, 6));
        assert_eq!(tags::strip_display(&markup::plain(row)), "Go now");
    }

    /// The tag of a checkbox or a radio choice is not on screen, so a run does not count it.
    #[test]
    fn runs_do_not_count_the_tag_of_a_checkbox() {
        let sheets = Sheets::default();
        for row in [
            "<checkbox>Go <b>bold</b>",
            "<checkbox checked>Go <b>bold</b>",
            "<checked>Go <b>bold</b>",
        ] {
            let style = sheets.row_style(Some(row), false, &StyleEnv::default());
            let shown = tags::strip_display(&markup::plain(row));
            assert_eq!(shown, "Go bold");
            let bold: String = shown
                .chars()
                .skip(style.runs[0].start)
                .take(style.runs[0].end - style.runs[0].start)
                .collect();
            assert_eq!(bold, "bold", "{row} {:?}", style.runs);
        }
    }

    /// An `<img>` shows its alt and its path, which is what the runs after it count past.
    #[test]
    fn runs_count_past_a_real_img_and_a() {
        let sheets = Sheets::default();
        let row = r#"<img src="a.jpg" alt="Cat"> x <b>bold</b>"#;
        let style = sheets.row_style(Some(row), false, &StyleEnv::default());
        let shown = tags::strip_display(&markup::plain(row));
        let bold: String = shown
            .chars()
            .skip(style.runs[0].start)
            .take(style.runs[0].end - style.runs[0].start)
            .collect();
        assert_eq!(bold, "bold", "{shown:?} {:?}", style.runs);
        let row = r#"<a href="u">Home</a> then <b>this</b>"#;
        let style = sheets.row_style(Some(row), false, &StyleEnv::default());
        let shown = tags::strip_display(&markup::plain(row));
        let text = |run: &TextRun| -> String {
            shown
                .chars()
                .skip(run.start)
                .take(run.end - run.start)
                .collect()
        };
        // The link stands out too now, so it is the first run and `<b>` the last.
        assert_eq!(style.runs.len(), 2, "{:?}", style.runs);
        assert_eq!(text(&style.runs[0]), "Home u", "{shown:?}");
        assert_eq!(text(&style.runs[1]), "this", "{shown:?}");
    }

    #[test]
    fn a_run_inside_a_whole_row_element_counts_from_its_start() {
        let sheets = Sheets::default();
        let style = sheets.row_style(Some("<p>ab <b>cd</b></p>"), false, &StyleEnv::default());
        assert_eq!((style.runs[0].start, style.runs[0].end), (3, 5));
    }

    #[test]
    fn a_list_takes_the_classes_of_the_row_it_belongs_to() {
        let mut sheets = sheets_with("ul.album { max-width: none } ul.album li { color: #111 }");
        let env = StyleEnv::default();
        assert_eq!(sheets.list_style(&env).max_width, Some(Length::Ch(120.0)));
        sheets.enter_list(&id(&[0]), true, vec!["album".into()], |_| None);
        assert_eq!(sheets.list_style(&env).max_width, None);
        assert_eq!(
            sheets.row_style(None, false, &env).li.color,
            Some(0x111111FF)
        );
    }

    /// A `<style>` row styles its list and everything below, and nothing else.
    #[test]
    fn a_scoped_sheet_applies_to_its_list_and_below() {
        let mut sheets = Sheets::default();
        let env = StyleEnv::default();
        let root = id(&[0, 3]);
        sheets.set_scoped(&root, "site", "li { color: #111 }".into());
        let key_at = |_: &IdArray| Some("site".to_owned());
        sheets.enter_list(&id(&[0, 3]), true, vec![], key_at);
        assert_eq!(
            sheets.row_style(None, false, &env).li.color,
            Some(0x111111FF)
        );
        sheets.enter_list(&id(&[0, 3, 1]), true, vec![], key_at);
        assert_eq!(
            sheets.row_style(None, false, &env).li.color,
            Some(0x111111FF)
        );
        sheets.enter_list(&id(&[0, 4]), true, vec![], key_at);
        assert_eq!(sheets.row_style(None, false, &env).li.color, None);
        sheets.enter_list(&id(&[0]), true, vec![], key_at);
        assert_eq!(sheets.row_style(None, false, &env).li.color, None);
    }

    /// Something else moved into its place: the sheet must not land on that.
    #[test]
    fn a_scoped_sheet_goes_with_the_element_it_belonged_to() {
        let mut sheets = Sheets::default();
        let env = StyleEnv::default();
        let root = id(&[0, 3]);
        sheets.set_scoped(&root, "site", "li { color: #111 }".into());
        sheets.enter_list(&root, true, vec![], |_| Some("something else".to_owned()));
        assert_eq!(sheets.row_style(None, false, &env).li.color, None);
        sheets.enter_list(&root, true, vec![], |_| Some("site".to_owned()));
        assert_eq!(
            sheets.row_style(None, false, &env).li.color,
            None,
            "forgotten"
        );
    }

    #[test]
    fn a_changed_style_row_replaces_the_old_sheet() {
        let mut sheets = Sheets::default();
        let env = StyleEnv::default();
        let root = id(&[0]);
        let key_at = |_: &IdArray| Some("k".to_owned());
        sheets.set_scoped(&root, "k", "li { color: #111 }".into());
        sheets.enter_list(&root, true, vec![], key_at);
        assert_eq!(
            sheets.row_style(None, false, &env).li.color,
            Some(0x111111FF)
        );
        sheets.set_scoped(&root, "k", "li { color: #222 }".into());
        sheets.enter_list(&root, true, vec![], key_at);
        assert_eq!(
            sheets.row_style(None, false, &env).li.color,
            Some(0x222222FF)
        );
    }

    #[test]
    fn a_document_is_forgotten_when_its_row_changes() {
        let mut sheets = Sheets::default();
        let root = id(&[0, 1]);
        sheets.mark_document(&root, "link row");
        assert_eq!(
            sheets.live_documents(|_| Some("link row".to_owned())),
            vec![root.clone()]
        );
        assert!(
            sheets
                .live_documents(|_| Some("other".to_owned()))
                .is_empty()
        );
        assert!(
            sheets
                .live_documents(|_| Some("link row".to_owned()))
                .is_empty()
        );
    }

    #[test]
    fn contrast_is_the_wcag_formula() {
        assert!((contrast(0xFFFFFFFF, 0x000000FF) - 21.0).abs() < 0.01);
        assert!((contrast(0x777777FF, 0x777777FF) - 1.0).abs() < 0.001);
    }

    #[test]
    fn unreadable_text_falls_back_to_the_palette() {
        let (white, black, near_black) = (0xFFFFFFFF, 0x000000FF, 0x111111FF);
        assert_eq!(readable_color(Some(white), black, 0xAAAAAAFF), white);
        assert_eq!(readable_color(Some(near_black), black, white), white);
        assert_eq!(readable_color(None, black, white), white);
    }

    #[test]
    fn a_cursor_that_would_not_show_falls_back_to_the_palette() {
        let (list, palette) = (0x000000FF, 0x2D4A28FF);
        assert_eq!(visible_focus(Some(0x234567FF), list, palette), 0x234567FF);
        assert_eq!(
            visible_focus(Some(list), list, palette),
            palette,
            "invisible"
        );
        assert_eq!(
            visible_focus(Some(0x234567AA), list, palette),
            palette,
            "see-through"
        );
        assert_eq!(visible_focus(None, list, palette), palette);
    }
}
