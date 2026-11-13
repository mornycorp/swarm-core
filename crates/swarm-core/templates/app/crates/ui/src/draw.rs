//! Drawing: the shell's state as egui widgets, and the user's clicks turned into calls on the shell.
//! Nothing here reads a file, starts a program or waits for a call: it draws memory, and a click
//! asks the shell to start a worker or to set the UI's state. Its colours are the theme's
//! ([`Theme`]), or a command's result's made egui's by the theme ([`Theme::parse`]), and its words
//! the catalog's ([`catalog`]): `scan` finds a colour or a word made here.

use egui::{Panel, RichText, Ui};

use crate::catalog;
use crate::shell::{Call, MenuNode, Row, Shell, item_row};
use crate::theme::Theme;
use crate::views::{Shown, Swatch};

/// A swatch's side, in points.
const SWATCH: f32 = 24.0;

/// Where each part of the window keeps its egui state: a typed id, never a string.
#[derive(Debug, Hash)]
enum Part {
    Menu,
    Commands,
    Outcome,
    Result,
}

/// The whole window: the menu bar on top, the command list on the left (when the UI's state shows
/// it), the selected command on the right.
pub(crate) fn shell(shell: &mut Shell, ui: &mut Ui) {
    let theme = Theme::of(ui.visuals());
    Panel::top(Part::Menu).show(ui, |ui| menu_bar(shell, ui));
    if shell.state().show_list {
        Panel::left(Part::Commands).resizable(true).default_size(260.0).show(ui, |ui| list(shell, &theme, ui));
    }
    egui::CentralPanel::default().show(ui, |ui| detail(shell, &theme, ui));
}

/// The menu bar the shell made from the commands' menu paths; a click activates the command.
fn menu_bar(shell: &mut Shell, ui: &mut Ui) {
    let clicked = egui::MenuBar::new().ui(ui, |ui| menu_level(shell.menus(), shell.rows(), ui)).inner;
    if let Some(at) = clicked {
        shell.activate(at);
    }
}

/// One level of a menu: a submenu per menu, an item per command (greyed out, with why, when it
/// can't run now). The row clicked. The depth is a menu path's, which the registry caps.
fn menu_level(nodes: &[MenuNode], rows: &[Row], ui: &mut Ui) -> Option<usize> {
    let mut clicked = None;
    for node in nodes {
        let hit = match item_row(node, rows) {
            Some((at, row)) => menu_item(node.label, row, ui).then_some(at),
            None => ui.menu_button(node.label, |ui| menu_level(&node.children, rows, ui)).inner.flatten(),
        };
        clicked = clicked.or(hit);
    }
    clicked
}

/// A command's menu item, with its shortcut: whether it was clicked.
fn menu_item(label: &str, row: &Row, ui: &mut Ui) -> bool {
    let mut button = egui::Button::new(label);
    if let Some(shortcut) = row.shortcut {
        button = button.shortcut_text(shortcut);
    }
    let response = ui.add_enabled(row.why.is_none(), button).on_hover_text(row.summary);
    match &row.why {
        Some(why) => response.on_disabled_hover_text(catalog::why_not(&why.message, &why.remediation)).clicked(),
        None => response.clicked(),
    }
}

/// The command list: a selectable line for each (in the `muted` colour when the UI has no view for
/// it or it can't run now), and a button that hides it.
fn list(shell: &mut Shell, theme: &Theme, ui: &mut Ui) {
    ui.horizontal(|ui| {
        ui.heading(catalog::COMMANDS);
        if ui.small_button(catalog::HIDE_LIST).clicked() {
            shell.set_show_list(false);
        }
    });
    let clicked = egui::ScrollArea::vertical()
        .show(ui, |ui| {
            let selected = shell.selected().map(|row| row.id);
            shell
                .rows()
                .iter()
                .enumerate()
                .filter_map(|(at, row)| ui.selectable_label(selected == Some(row.id), label(row, theme)).on_hover_text(row.summary).clicked().then_some(at))
                .last()
        })
        .inner;
    if let Some(at) = clicked {
        shell.select(at);
    }
}

fn label(row: &Row, theme: &Theme) -> RichText {
    let text = RichText::new(catalog::row(row.title, row.id));
    if row.viewable && row.why.is_none() { text } else { text.color(theme.muted) }
}

/// The selected command: its summary, the consent box, Run and Cancel, and the call's state.
fn detail(shell: &mut Shell, theme: &Theme, ui: &mut Ui) {
    if !shell.state().show_list && ui.button(catalog::SHOW_LIST).clicked() {
        shell.set_show_list(true);
    }
    let Some(row) = shell.selected().cloned() else {
        ui.label(catalog::SELECT_A_COMMAND);
        return;
    };
    ui.heading(row.title);
    ui.label(row.summary);
    ui.monospace(row.id);
    ui.separator();
    controls(shell, &row, theme, ui);
    ui.separator();
    egui::ScrollArea::vertical().id_salt(Part::Outcome).show(ui, |ui| outcome(shell.call(), theme, ui));
}

fn controls(shell: &mut Shell, row: &Row, theme: &Theme, ui: &mut Ui) {
    if row.needs_consent {
        let mut consent = shell.consent();
        if ui.checkbox(&mut consent, catalog::CONSENT).changed() {
            shell.set_consent(consent);
        }
    }
    if !row.viewable {
        ui.label(catalog::NO_VIEW);
        return;
    }
    if let Some(why) = &row.why {
        ui.colored_label(theme.error, catalog::not_available(&why.message));
        ui.label(catalog::fix(&why.remediation));
    }
    ui.horizontal(|ui| {
        if ui.add_enabled(!shell.is_running() && row.why.is_none(), egui::Button::new(catalog::RUN)).clicked() {
            shell.run_selected();
        }
        if shell.is_running() {
            if ui.button(catalog::CANCEL).clicked() {
                shell.cancel();
            }
            ui.spinner();
        }
    });
}

/// What the last call gave: its typed result's lines, or the error with what to do about it.
fn outcome(call: &Call, theme: &Theme, ui: &mut Ui) {
    match call {
        Call::Idle => {}
        Call::Running { id, .. } => {
            ui.label(catalog::running(id));
        }
        Call::Done { shown, .. } => {
            fields(shown, ui);
            swatches(shown, theme, ui);
        }
        Call::Failed { error, .. } => {
            ui.colored_label(theme.error, catalog::failed(error.code.as_str(), &error.message));
            ui.label(catalog::fix(&error.remediation));
            if let Some(key) = &error.key {
                ui.label(catalog::param(key));
            }
        }
    }
}

/// A result's lines: a label and its value a row, in the order the view gave them.
fn fields(shown: &Shown, ui: &mut Ui) {
    egui::Grid::new(Part::Result).num_columns(2).striped(true).show(ui, |ui| {
        for (label, value) in &shown.fields {
            ui.label(RichText::new(label).strong());
            ui.monospace(value);
            ui.end_row();
        }
    });
}

/// A result's colours: each row's label and lines, then its squares, wrapping to the width.
fn swatches(shown: &Shown, theme: &Theme, ui: &mut Ui) {
    for row in &shown.swatches {
        ui.add_space(SWATCH / 2.0);
        ui.label(RichText::new(&row.label).strong());
        for line in &row.lines {
            ui.label(line);
        }
        ui.horizontal_wrapped(|ui| row.swatches.iter().for_each(|s| swatch(s, theme, ui)));
    }
}

/// A square of the swatch's colour in the window's look, edged in a range entry's border, and under
/// it the entry's number, on the entry made a background for text, or a token's value.
///
/// The square and its text are one cell, sized before it is placed, so the wrapping row it is in
/// moves it to the next line when it doesn't fit. A child `Ui` (`ui.vertical`) is placed without
/// that check, and a range's row ran off the view's right edge (mornycorp/swarm-core#102).
fn swatch(swatch: &Swatch, theme: &Theme, ui: &mut Ui) {
    let (text, behind) = match &swatch.number {
        Some(number) => (RichText::new(&number.index).color(theme.strong), Some(theme.parse(number.behind_text.of(theme.look)))),
        None => (RichText::new(swatch.colour.of(theme.look)).monospace(), None),
    };
    let text = egui::WidgetText::from(text).into_galley(ui, Some(egui::TextWrapMode::Extend), f32::INFINITY, egui::TextStyle::Body);
    let cell = egui::vec2(SWATCH.max(text.size().x), SWATCH + ui.spacing().item_spacing.y + text.size().y);
    ui.allocate_ui_with_layout(cell, egui::Layout::top_down(egui::Align::Min), |ui| {
        let (square, _) = ui.allocate_exact_size(egui::Vec2::splat(SWATCH), egui::Sense::hover());
        let fill = theme.parse(swatch.colour.of(theme.look));
        let edge = swatch.border.as_ref().map_or(egui::Stroke::NONE, |border| egui::Stroke::new(1.0, theme.parse(border.of(theme.look))));
        ui.painter().rect(square, 0.0, fill, edge, egui::StrokeKind::Inside);
        match behind {
            Some(fill) => {
                egui::Frame::NONE.fill(fill).show(ui, |ui| ui.label(text));
            }
            None => {
                ui.label(text);
            }
        }
    });
}
