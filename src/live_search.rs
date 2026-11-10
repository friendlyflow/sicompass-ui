//! Ctrl+F below what the user has opened.
//!
//! Extended search lists the tree in memory, and most providers fill a folder
//! only when the user steps into it, so on its own it finds the current list
//! and whatever was opened before. Two ways reach deeper:
//!
//! - **The provider's own walk** (`Provider::start_extended_search`). It runs
//!   on the provider's thread, so the window goes on drawing and taking keys,
//!   and what it finds is added to the list as it arrives ([`drain`]). Its rows
//!   carry a `nav_path`, and Enter goes there with `navigate_to_path`.
//! - **Otherwise, the levels scroll mode loads** (`prefetch_subtree`): a few
//!   levels, within a size and time budget, added to the tree first.
//!
//! The rows already in the tree come first and are kept: they open on the
//! focused element, and show the provider's own labels. A walk's row for an
//! element the tree already shows is left out.

use std::collections::HashSet;

use crate::app_state::{AppRenderer, Coordinate, RenderListItem};
use sicompass_sdk::SearchResultItem;
use sicompass_sdk::ffon::{FfonElement, IdArray, get_ffon_at_id};

/// How often the walk's results are taken while it runs. A take is a call to
/// the provider, and adding rows re-filters the whole list.
const TAKE_EVERY_MS: u64 = 100;

/// A walk that ends sooner is not announced: it ended before the screen reader
/// finished saying the mode's name, and the count already shows it.
const ANNOUNCE_AFTER_MS: u64 = 1000;

/// The last index of a walk's row id. The id's list is the one the walk
/// started from, so the row belongs to that provider and level, but no element
/// is there: whatever looks a row up in the tree (its style, Ctrl+C, a
/// checkbox) finds nothing, and the cursor id is never set to it.
pub const NOT_IN_TREE: usize = usize::MAX;

/// The walk behind an open Ctrl+F.
pub struct LiveSearch {
    provider_idx: usize,
    /// `current_id` where the walk started.
    base_id: IdArray,
    /// The provider's path there: the folder the walk lists.
    root_path: String,
    /// What the walk found that the tree does not show, in the order found.
    rows: Vec<RenderListItem>,
    /// The paths of the rows the tree shows, refreshed with every rebuild.
    in_tree: HashSet<String>,
    done: bool,
    /// It ended before it had seen everything (its budget).
    cut_short: bool,
    started: u64,
    last_take: u64,
}

/// Where the walk behind the open search stands, for the header and speech.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WalkStatus {
    /// Still looking: more rows may come.
    Searching,
    /// Over, and finished.
    Finished,
    /// Over, but stopped at its limit: the results are incomplete.
    CutShort,
}

/// The walk behind the open search, if there is one.
pub fn status(r: &AppRenderer) -> Option<WalkStatus> {
    let live = r.live_search.as_ref()?;
    Some(match (live.done, live.cut_short) {
        (false, _) => WalkStatus::Searching,
        (true, false) => WalkStatus::Finished,
        (true, true) => WalkStatus::CutShort,
    })
}

/// What the header adds to the result count: `, searching` while the walk
/// runs, `, stopped at the limit` when it was cut short, nothing otherwise.
pub fn header_suffix(r: &AppRenderer) -> String {
    match status(r) {
        Some(WalkStatus::Searching) => {
            format!(", {}", sicompass_sdk::localize::t("search-searching"))
        }
        Some(WalkStatus::CutShort) => {
            format!(
                ", {}",
                sicompass_sdk::localize::t("search-stopped-at-limit")
            )
        }
        Some(WalkStatus::Finished) | None => String::new(),
    }
}

/// Whether a row came from a walk rather than from the tree.
pub fn is_walk_row(item: &RenderListItem) -> bool {
    item.id.last() == Some(NOT_IN_TREE)
}

/// Before an extended search is built from where the cursor is now: Ctrl+F,
/// and a step left or right inside the search. Starts the active provider's
/// walk, or else loads the levels below the way scroll mode does.
pub fn prepare(r: &mut AppRenderer) {
    stop(r);
    if r.search_in_manual || r.current_id.depth() < 2 {
        return;
    }
    let Some(idx) = crate::provider::active_provider_index(r).filter(|&i| i < r.providers.len())
    else {
        return;
    };
    if r.providers[idx].start_extended_search() {
        r.live_search = Some(LiveSearch {
            provider_idx: idx,
            base_id: r.current_id.clone(),
            root_path: crate::provider::current_path(r).to_owned(),
            rows: Vec::new(),
            in_tree: HashSet::new(),
            done: false,
            cut_short: false,
            started: crate::handlers::sdl_ticks(),
            last_take: 0,
        });
        return;
    }
    crate::handlers::prefetch_subtree(r);
}

/// The search is over, or starts again from elsewhere: stop the walk.
pub fn stop(r: &mut AppRenderer) {
    if let Some(live) = r.live_search.take()
        && !live.done
        && let Some(p) = r.providers.get_mut(live.provider_idx)
    {
        p.stop_extended_search();
    }
}

/// Add the walk's rows after the tree's, in `create_list_extended_search`.
pub(crate) fn append_rows(r: &mut AppRenderer) {
    let Some(live) = r.live_search.as_mut() else {
        return;
    };
    live.in_tree = tree_paths(&r.ffon, &live.base_id, &live.root_path);
    let in_tree = &live.in_tree;
    r.total_list.extend(
        live.rows
            .iter()
            .filter(|row| {
                !row.nav_path
                    .as_deref()
                    .is_some_and(|p| in_tree.contains(&normalize(p)))
            })
            .cloned(),
    );
}

/// Every frame: take what the walk found since the last time and add it to
/// the list, keeping the cursor on the row it is on. Stops the walk once the
/// user has left the search, whichever way they left.
pub fn drain(r: &mut AppRenderer) {
    let Some(live) = r.live_search.as_ref() else {
        return;
    };
    if r.coordinate.base() != Coordinate::ExtendedSearch || r.search_in_manual {
        stop(r);
        return;
    }
    if live.done {
        return;
    }
    let now = crate::handlers::sdl_ticks();
    if now.saturating_sub(live.last_take) < TAKE_EVERY_MS {
        return;
    }
    let idx = live.provider_idx;
    let Some(provider) = r.providers.get_mut(idx) else {
        r.live_search = None;
        return;
    };
    let progress = provider.take_extended_search_items();
    let Some(live) = r.live_search.as_mut() else {
        return;
    };
    live.last_take = now;
    live.done = progress.done;
    live.cut_short = progress.done && progress.cut_short;
    let announce = progress.done && now.saturating_sub(live.started) >= ANNOUNCE_AFTER_MS;
    let rows: Vec<RenderListItem> = progress
        .items
        .into_iter()
        .map(|f| live.row(f))
        .filter(|row| {
            !row.nav_path
                .as_deref()
                .is_some_and(|p| live.in_tree.contains(&normalize(p)))
        })
        .collect();
    live.rows.extend(rows.iter().cloned());
    if !rows.is_empty() {
        let selected = selected_row(r);
        r.total_list.extend(rows);
        if !r.input_buffer.is_empty() {
            let query = r.input_buffer.clone();
            crate::list::populate_list_current_layer(r, &query);
            if let Some(raw) = selected
                && let Some(pos) = r.filtered_list_indices.iter().position(|&i| i == raw)
            {
                r.list_index = pos;
            }
        }
    }
    if announce {
        announce_end(r);
    }
    r.needs_redraw = true;
}

/// Say that the walk is over, and how many rows the search shows now.
fn announce_end(r: &mut AppRenderer) {
    let key = match status(r) {
        Some(WalkStatus::CutShort) => "search-finished-at-limit",
        _ => "search-finished",
    };
    let mut args = sicompass_sdk::localize::Args::new();
    args.set("count", r.active_list_len() as i64);
    let text = sicompass_sdk::localize::t_args(key, &args);
    r.announce_provider_line(text);
}

/// Let the next [`drain`] take at once, whenever the last one was. For tests,
/// which have no frames to wait for.
#[cfg(test)]
pub(crate) fn take_next_frame(r: &mut AppRenderer) {
    if let Some(live) = r.live_search.as_mut() {
        live.last_take = 0;
    }
}

/// Make the walk a long one, as if it had started a minute ago. For tests.
#[cfg(test)]
pub(crate) fn started_long_ago(r: &mut AppRenderer) {
    if let Some(live) = r.live_search.as_mut() {
        live.started = live.started.saturating_sub(60_000);
    }
}

impl LiveSearch {
    fn row(&self, found: SearchResultItem) -> RenderListItem {
        let mut id = self.base_id.clone();
        id.set_last(NOT_IN_TREE);
        RenderListItem {
            id,
            label: found.label,
            data: (!found.breadcrumb.is_empty()).then_some(found.breadcrumb),
            nav_path: Some(found.nav_path),
            ext_prefix: None,
        }
    }
}

/// The index in `total_list` of the row the cursor is on.
fn selected_row(r: &AppRenderer) -> Option<usize> {
    if r.search_found_nothing() {
        None
    } else if r.filtered_list_indices.is_empty() {
        Some(r.list_index)
    } else {
        r.filtered_list_indices.get(r.list_index).copied()
    }
}

/// The paths of the elements in the list at `base_id` and below, built the way
/// the provider builds a walk's `nav_path` (its path, then each element's
/// name), so the walk's rows for them can be left out.
fn tree_paths(ffon: &[FfonElement], base_id: &IdArray, root_path: &str) -> HashSet<String> {
    fn walk(list: &[FfonElement], path: &str, out: &mut HashSet<String>) {
        for elem in list {
            let raw = match elem {
                FfonElement::Str(s) => s.as_str(),
                FfonElement::Obj(o) => o.key.as_str(),
            };
            let name = sicompass_sdk::tags::unescape(&crate::provider::element_nav_name(raw));
            if name.is_empty() {
                continue;
            }
            let child = join(path, &name);
            if let FfonElement::Obj(o) = elem {
                walk(&o.children, &child, out);
            }
            out.insert(normalize(&child));
        }
    }
    let mut out = HashSet::new();
    if let Some(list) = get_ffon_at_id(ffon, base_id) {
        walk(list, root_path, &mut out);
    }
    out
}

fn join(dir: &str, name: &str) -> String {
    if dir.ends_with(['/', '\\']) {
        format!("{dir}{name}")
    } else {
        format!("{dir}/{name}")
    }
}

/// One spelling for a path from either side: Windows joins with `\`.
fn normalize(path: &str) -> String {
    path.replace('\\', "/")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tree_paths_follow_the_names_below_the_list() {
        let mut docs = sicompass_sdk::ffon::FfonObject::new("<input>docs</input>");
        docs.push(FfonElement::new_str(format!(
            "<input>{}</input>",
            sicompass_sdk::tags::escape("a <b>.txt")
        )));
        let tree = vec![FfonElement::Obj({
            let mut root = sicompass_sdk::ffon::FfonObject::new("Files");
            root.push(FfonElement::Obj(docs));
            root.push(FfonElement::new_str("<input>top.txt</input>"));
            root
        })];
        let mut base = IdArray::new();
        base.push(0);
        base.push(0);
        let paths = tree_paths(&tree, &base, "/home/u/");
        assert!(paths.contains("/home/u/docs"));
        assert!(paths.contains("/home/u/top.txt"));
        // The name the file has, not the escaped one the row carries.
        assert!(paths.contains("/home/u/docs/a <b>.txt"), "{paths:?}");
        assert_eq!(paths.len(), 3);
    }

    #[test]
    fn a_windows_path_reads_like_any_other() {
        assert_eq!(normalize(r"C:\Users\u\x.txt"), "C:/Users/u/x.txt");
        assert_eq!(join(r"C:\", "x"), r"C:\x");
        assert_eq!(join("/a", "x"), "/a/x");
    }
}
