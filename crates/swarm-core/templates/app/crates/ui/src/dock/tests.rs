use super::*;

fn tabs(panes: &[&str], active: u32) -> DockNode {
    DockNode::Tabs { panes: panes.iter().map(|p| (*p).to_owned()).collect(), active }
}

fn split(across: bool, parts: Vec<(u16, DockNode)>) -> DockNode {
    DockNode::Split { across, parts: parts.into_iter().map(|(share, node)| DockPart { share, node }).collect() }
}

/// Every pane a layout shows, in order.
fn panes_of(node: &DockNode) -> Vec<String> {
    match node {
        DockNode::Tabs { panes, .. } => panes.clone(),
        DockNode::Split { parts, .. } => parts.iter().flat_map(|part| panes_of(&part.node)).collect(),
    }
}

/// The default layout builds and reads back as itself: the list a third of the width, the view
/// beside it, each in its tabs.
#[test]
fn the_default_layout_builds_and_reads_back_as_itself() {
    let layout = DockNode::default_layout();
    assert_eq!(layout_of(&build(&layout)), Some(layout));
}

/// A layout that is a person's arrangement reads back as built: tabs with the shown one, splits
/// across and down with their shares per mille.
#[test]
fn an_arrangement_reads_back_as_built() {
    let layout = split(false, vec![(250, tabs(&[PANE_VIEW, PANE_COMMANDS], 1)), (750, split(true, vec![]))]);
    let read = layout_of(&build(&layout)).unwrap();
    assert_eq!(read, tabs(&[PANE_VIEW, PANE_COMMANDS], 1), "an empty split is dropped, and a split of one part is that part");
    let two = split(false, vec![(250, tabs(&[PANE_VIEW], 0)), (750, tabs(&[PANE_COMMANDS], 0))]);
    assert_eq!(layout_of(&build(&two)), Some(two));
}

/// Whatever a layout holds, the dock shows each pane once: one left out is added, one named twice
/// is shown once, an id it doesn't know and a shown tab out of range are let go, and what it reads
/// back is a layout `ui.set` takes.
#[test]
fn any_layout_shows_each_pane_once() {
    let odd = [
        tabs(&[PANE_VIEW], 0),
        tabs(&[PANE_VIEW, PANE_VIEW, PANE_COMMANDS], 7),
        tabs(&["nope"], 0),
        split(true, vec![(1, tabs(&[PANE_COMMANDS], 0)), (1000, tabs(&[PANE_COMMANDS, PANE_VIEW], 0))]),
        split(true, vec![(1, split(false, vec![(1, split(true, vec![(1, tabs(&[PANE_VIEW], 0))]))]))]),
    ];
    for layout in odd {
        let read = layout_of(&build(&layout)).unwrap_or_else(|| panic!("{layout:?} builds no dock"));
        let mut shown = panes_of(&read);
        shown.sort();
        assert_eq!(shown, [PANE_COMMANDS, PANE_VIEW], "{layout:?} shows {read:?}");
        assert!(read.check().is_ok(), "{read:?} is a layout ui.set takes");
    }
}

/// A part's share is kept per mille of its split, and never 0.
#[test]
fn shares_are_per_mille_and_never_0() {
    assert_eq!((per_mille(1.0, 4.0), per_mille(3.0, 4.0)), (250, 750));
    assert_eq!((per_mille(0.0, 4.0), per_mille(f32::NAN, 4.0), per_mille(1.0, 0.0)), (1, 1, 1));
    assert_eq!(per_mille(5.0, 4.0), 1000);
}

/// The dock draws in the tokens: the shown tab joins its pane, the others sit on the bar with muted
/// titles, and a separator lights up in the accent when it is hovered or dragged.
#[test]
fn the_dock_draws_in_the_tokens() {
    for visuals in [egui::Visuals::dark(), egui::Visuals::light()] {
        let theme = Theme::of(&visuals);
        assert_eq!((tab_fill(&theme, true), tab_fill(&theme, false)), (theme.background, theme.header));
        assert_eq!((tab_text(&theme, true), tab_text(&theme, false)), (theme.text, theme.muted));
        assert_eq!(separator(&theme, ResizeState::Idle), theme.chrome);
        assert_eq!((separator(&theme, ResizeState::Hovering), separator(&theme, ResizeState::Dragging)), (theme.accent, theme.accent));
    }
    assert_eq!(TAB_BAR_HEIGHT, density::ROW_HEIGHT + 4.0);
}

/// Each pane has a title and an icon, and the ids are the engine's.
#[test]
fn each_pane_has_its_id_title_and_icon() {
    for pane in Pane::ALL {
        assert_eq!(Pane::of(pane.id()), Some(pane));
        assert!(!pane.title().is_empty());
        assert!(icons::ALL.contains(&icons::for_pane(pane)));
    }
    assert_eq!(Pane::ALL.map(Pane::id), {{crate_prefix_ident}}engine::PANES);
}
