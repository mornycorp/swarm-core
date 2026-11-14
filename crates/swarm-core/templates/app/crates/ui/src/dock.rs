//! The dock (the App UI Substrate › The dock): the window's panes as `egui_tiles` tiles, tabs and
//! splits a person can drag and resize, styled from the theme's tokens.
//!
//! **The UI state is the layout's source of truth** ([`DockNode`], the session's, which `ui.set` and
//! `ui.inspect` set and read). Each frame [`show`] builds the tiles from it when it changed since they
//! were built (a `ui.set` from an agent, a restored launch), draws them, and reads the arrangement
//! back: what a person dragged or resized goes to the state through `ui.set`, as an agent's change
//! would. egui_tiles' own saved state is never loaded, and a layout builds whatever it holds: a pane
//! it leaves out is added beside it, and one it names twice is shown once.

use egui::{Color32, Frame, Margin, Stroke, Ui, Visuals, WidgetText};
use egui_tiles::{Behavior, Container, LinearDir, ResizeState, SimplificationOptions, TabState, Tile, TileId, Tiles, Tree, UiResponse};
use {{crate_prefix_ident}}engine::{DockNode, DockPart, PANE_COMMANDS, PANE_VIEW};

use crate::catalog;
use crate::density;
use crate::draw;
use crate::icons;
use crate::shell::Shell;
use crate::theme::Theme;

/// Between panes, and around the dock, in points.
pub const GAP: f32 = 4.0;
/// [`GAP`] as a margin's whole points.
pub const GAP_MARGIN: i8 = 4;
/// A tab bar's height: a row and a little.
pub const TAB_BAR_HEIGHT: f32 = density::ROW_HEIGHT + 4.0;
/// Inside a pane, around what it draws.
pub const PANE_MARGIN: i8 = 6;
/// A part's share as the layout keeps it: per mille of its split.
const PER_MILLE: f32 = 1000.0;

/// The dock's panes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Pane {
    /// The command list.
    Commands,
    /// The open command's view.
    View,
}

impl Pane {
    pub const ALL: [Pane; 2] = [Pane::Commands, Pane::View];

    /// Its id in the layout.
    pub fn id(self) -> &'static str {
        match self {
            Pane::Commands => PANE_COMMANDS,
            Pane::View => PANE_VIEW,
        }
    }

    /// The pane with the id `id`, if the dock has one.
    pub fn of(id: &str) -> Option<Pane> {
        Pane::ALL.into_iter().find(|pane| pane.id() == id)
    }

    /// Its tab's title.
    pub fn title(self) -> &'static str {
        match self {
            Pane::Commands => catalog::COMMANDS,
            Pane::View => catalog::VIEW_PANE,
        }
    }
}

/// Where egui keeps the dock's state: a typed salt, never a string.
#[derive(Debug, Hash)]
struct DockSalt;

/// The tiles the dock draws, the layout they were built from, the layout as last read back, and the
/// pane a person last clicked in (its tab is underlined).
#[derive(Debug)]
pub struct Dock {
    built_from: DockNode,
    last: Option<DockNode>,
    tree: Tree<Pane>,
    focused: Option<Pane>,
}

impl Dock {
    /// The tiles of `layout`.
    pub fn of(layout: &DockNode) -> Dock {
        let tree = build(layout);
        Dock { built_from: layout.clone(), last: layout_of(&tree), tree, focused: None }
    }

    /// The layout the tiles show now.
    pub fn layout(&self) -> Option<DockNode> {
        layout_of(&self.tree)
    }

    /// The tiles, to change as a drag would (a test's).
    pub fn tree_mut(&mut self) -> &mut Tree<Pane> {
        &mut self.tree
    }
}

/// The tiles of `layout`: each pane it names once, in its tabs and splits, and each it leaves out
/// beside them.
pub fn build(layout: &DockNode) -> Tree<Pane> {
    let mut tiles = Tiles::default();
    let mut placed = Vec::new();
    let root = insert(&mut tiles, layout, &mut placed);
    let missing: Vec<TileId> = Pane::ALL
        .into_iter()
        .filter(|pane| !placed.contains(pane))
        .map(|pane| {
            let tile = tiles.insert_pane(pane);
            tiles.insert_tab_tile(vec![tile])
        })
        .collect();
    let root = match root {
        Some(root) if missing.is_empty() => root,
        root => tiles.insert_horizontal_tile(root.into_iter().chain(missing).collect()),
    };
    Tree::new(egui::Id::unique(DockSalt), root, tiles)
}

fn insert(tiles: &mut Tiles<Pane>, node: &DockNode, placed: &mut Vec<Pane>) -> Option<TileId> {
    match node {
        DockNode::Tabs { panes, active } => insert_tabs(tiles, panes, *active, placed),
        DockNode::Split { across, parts } => insert_split(tiles, *across, parts, placed),
    }
}

fn insert_tabs(tiles: &mut Tiles<Pane>, panes: &[String], active: u32, placed: &mut Vec<Pane>) -> Option<TileId> {
    let mut children = Vec::new();
    for pane in panes.iter().filter_map(|id| Pane::of(id)) {
        if !placed.contains(&pane) {
            placed.push(pane);
            children.push(tiles.insert_pane(pane));
        }
    }
    let shown = usize::try_from(active).ok().and_then(|at| children.get(at)).copied();
    if children.is_empty() {
        return None;
    }
    let tabs = tiles.insert_tab_tile(children);
    if let (Some(child), Some(Tile::Container(Container::Tabs(t)))) = (shown, tiles.get_mut(tabs)) {
        t.set_active(child);
    }
    Some(tabs)
}

fn insert_split(tiles: &mut Tiles<Pane>, across: bool, parts: &[DockPart], placed: &mut Vec<Pane>) -> Option<TileId> {
    let parts: Vec<(TileId, u16)> = parts.iter().filter_map(|part| Some((insert(tiles, &part.node, placed)?, part.share))).collect();
    if let [(only, _)] = parts.as_slice() {
        return Some(*only);
    }
    let children: Vec<TileId> = parts.iter().map(|&(tile, _)| tile).collect();
    if children.is_empty() {
        return None;
    }
    let split = if across { tiles.insert_horizontal_tile(children) } else { tiles.insert_vertical_tile(children) };
    if let Some(Tile::Container(Container::Linear(linear))) = tiles.get_mut(split) {
        for &(tile, share) in &parts {
            linear.shares.set_share(tile, f32::from(share));
        }
    }
    Some(split)
}

/// The layout `tree` shows: its tabs and splits, each part's share per mille of its split.
pub fn layout_of(tree: &Tree<Pane>) -> Option<DockNode> {
    tree.root().and_then(|root| node_of(&tree.tiles, root))
}

fn node_of(tiles: &Tiles<Pane>, tile: TileId) -> Option<DockNode> {
    match tiles.get(tile)? {
        Tile::Pane(pane) => Some(DockNode::Tabs { panes: vec![pane.id().to_owned()], active: 0 }),
        Tile::Container(Container::Tabs(tabs)) => tabs_of(tiles, &tabs.children, tabs.active),
        Tile::Container(Container::Linear(linear)) => {
            let shares: Vec<(TileId, f32)> = linear.children.iter().map(|&child| (child, linear.shares[child])).collect();
            split_of(tiles, linear.dir == LinearDir::Horizontal, &shares)
        }
        Tile::Container(grid) => {
            let shares: Vec<(TileId, f32)> = grid.children().map(|&child| (child, 1.0)).collect();
            split_of(tiles, true, &shares)
        }
    }
}

fn tabs_of(tiles: &Tiles<Pane>, children: &[TileId], active: Option<TileId>) -> Option<DockNode> {
    let panes: Vec<(TileId, &'static str)> = children
        .iter()
        .filter_map(|&child| match tiles.get(child) {
            Some(Tile::Pane(pane)) => Some((child, pane.id())),
            _ => None,
        })
        .collect();
    let shown = panes.iter().position(|&(child, _)| Some(child) == active).unwrap_or(0);
    let active = u32::try_from(shown).unwrap_or(0);
    (!panes.is_empty()).then(|| DockNode::Tabs { panes: panes.into_iter().map(|(_, id)| id.to_owned()).collect(), active })
}

fn split_of(tiles: &Tiles<Pane>, across: bool, shares: &[(TileId, f32)]) -> Option<DockNode> {
    let total: f32 = shares.iter().map(|&(_, share)| share.max(0.0)).sum();
    let parts: Vec<DockPart> =
        shares.iter().filter_map(|&(child, share)| Some(DockPart { share: per_mille(share, total), node: node_of(tiles, child)? })).collect();
    match parts.len() {
        0 => None,
        1 => parts.into_iter().next().map(|part| part.node),
        _ => Some(DockNode::Split { across, parts }),
    }
}

/// `share` of `total`, per mille, at least 1: what a part's share is kept as.
fn per_mille(share: f32, total: f32) -> u16 {
    let fraction = if total > 0.0 && share.is_finite() { share / total } else { 0.0 };
    let mille = (fraction * PER_MILLE).round().clamp(1.0, PER_MILLE);
    // In 1..=1000 once clamped: the cast neither wraps nor truncates a whole part.
    mille as u16
}

/// Draws the dock in `ui`: builds the tiles again when the UI state's layout changed, hides the
/// command list when the state says so, and sets what a person dragged as the state's layout.
pub(crate) fn show(shell: &mut Shell, theme: &Theme, ui: &mut Ui) {
    let wanted = shell.state().dock.clone().unwrap_or_else(DockNode::default_layout);
    let mut dock = match shell.take_dock() {
        Some(dock) if dock.built_from == wanted => dock,
        _ => Dock::of(&wanted),
    };
    show_list(&mut dock.tree, shell.state().show_list);
    let mut panes = Panes { shell: &mut *shell, theme, focused: &mut dock.focused };
    dock.tree.ui(&mut panes, ui);
    let now = layout_of(&dock.tree);
    if now != dock.last {
        dock.last = now.clone();
        if let Some(layout) = now {
            dock.built_from = layout.clone();
            shell.set_dock(layout);
        }
    }
    shell.put_dock(dock);
}

/// Shows or hides the command list's tile, and its tabs when they hold nothing else.
fn show_list(tree: &mut Tree<Pane>, shown: bool) {
    let Some(list) = tree.tiles.iter().find(|(_, tile)| matches!(tile, Tile::Pane(Pane::Commands))).map(|(&id, _)| id) else { return };
    tree.tiles.set_visible(list, shown);
    if let Some(parent) = tree.tiles.parent_of(list) {
        let alone = matches!(tree.tiles.get(parent), Some(Tile::Container(c)) if c.children().count() == 1);
        if alone {
            tree.tiles.set_visible(parent, shown);
        }
    }
}

/// How the dock draws: each pane's contents, and its tabs, gaps and drag previews in the tokens.
struct Panes<'a> {
    shell: &'a mut Shell,
    theme: &'a Theme,
    focused: &'a mut Option<Pane>,
}

impl Behavior<Pane> for Panes<'_> {
    fn pane_ui(&mut self, ui: &mut Ui, _tile: TileId, pane: &mut Pane) -> UiResponse {
        if ui.input(|i| i.pointer.any_pressed()) && ui.rect_contains_pointer(ui.max_rect()) {
            *self.focused = Some(*pane);
        }
        let frame = Frame::NONE.fill(self.theme.background).stroke(Stroke::new(1.0, self.theme.frame)).inner_margin(Margin::same(PANE_MARGIN));
        frame.show(ui, |ui| {
            ui.set_min_size(ui.available_size());
            match pane {
                Pane::Commands => draw::list(self.shell, self.theme, ui),
                Pane::View => draw::detail(self.shell, self.theme, ui),
            }
        });
        UiResponse::None
    }

    fn tab_title_for_pane(&mut self, pane: &Pane) -> WidgetText {
        icons::titled(icons::for_pane(*pane), pane.title())
    }

    fn on_tab_button(&mut self, tiles: &mut Tiles<Pane>, tile: TileId, response: egui::Response) -> egui::Response {
        let focused = matches!(tiles.get(tile), Some(Tile::Pane(pane)) if Some(*pane) == *self.focused);
        if focused {
            let rect = response.rect;
            let line = egui::Rect::from_min_max(egui::pos2(rect.left(), rect.bottom() - 2.0), rect.right_bottom());
            response.ctx.layer_painter(response.layer_id).rect_filled(line, 0.0, self.theme.accent);
        }
        response
    }

    fn tab_bar_height(&self, _style: &egui::Style) -> f32 {
        TAB_BAR_HEIGHT
    }

    fn gap_width(&self, _style: &egui::Style) -> f32 {
        GAP
    }

    fn simplification_options(&self) -> SimplificationOptions {
        SimplificationOptions { all_panes_must_have_tabs: true, ..SimplificationOptions::default() }
    }

    fn tab_bar_color(&self, _visuals: &Visuals) -> Color32 {
        self.theme.header
    }

    fn tab_bg_color(&self, _visuals: &Visuals, _tiles: &Tiles<Pane>, _tile: TileId, state: &TabState) -> Color32 {
        tab_fill(self.theme, state.active)
    }

    fn tab_outline_stroke(&self, _visuals: &Visuals, _tiles: &Tiles<Pane>, _tile: TileId, state: &TabState) -> Stroke {
        if state.active { Stroke::new(1.0, self.theme.frame) } else { Stroke::NONE }
    }

    fn tab_bar_hline_stroke(&self, _visuals: &Visuals) -> Stroke {
        Stroke::new(1.0, self.theme.frame)
    }

    fn tab_text_color(&self, _visuals: &Visuals, _tiles: &Tiles<Pane>, _tile: TileId, state: &TabState) -> Color32 {
        tab_text(self.theme, state.active)
    }

    fn resize_stroke(&self, _style: &egui::Style, state: ResizeState) -> Stroke {
        Stroke::new(1.0, separator(self.theme, state))
    }

    fn dragged_overlay_color(&self, _visuals: &Visuals) -> Color32 {
        self.theme.accent.gamma_multiply(0.4)
    }

    fn drag_preview_stroke(&self, _visuals: &Visuals) -> Stroke {
        Stroke::new(1.0, self.theme.accent)
    }

    fn drag_preview_color(&self, _visuals: &Visuals) -> Color32 {
        self.theme.accent.gamma_multiply(0.15)
    }
}

/// A tab's fill: the shown tab joins its pane (`background`), the others sit on the bar (`header`).
pub fn tab_fill(theme: &Theme, active: bool) -> Color32 {
    if active { theme.background } else { theme.header }
}

/// A tab's title: `text` on the shown tab, `muted` on the others.
pub fn tab_text(theme: &Theme, active: bool) -> Color32 {
    if active { theme.text } else { theme.muted }
}

/// A separator between panes: `chrome` at rest, `accent` when hovered or dragged.
pub fn separator(theme: &Theme, state: ResizeState) -> Color32 {
    match state {
        ResizeState::Idle => theme.chrome,
        ResizeState::Hovering | ResizeState::Dragging => theme.accent,
    }
}

#[cfg(test)]
mod tests;
