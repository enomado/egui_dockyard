#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")] // hide console window on Windows in release

use eframe::{NativeOptions, egui};
use egui::{Color32, RichText};
use egui_dockyard::{DockArea, DockState, NodePath, Style};

fn main() -> eframe::Result<()> {
    let options = NativeOptions::default();
    eframe::run_native(
        "My egui App",
        options,
        Box::new(|_cc| Ok(Box::<MyApp>::default())),
    )
}

#[derive(Clone, Copy)]
enum MyTabKind {
    Regular,
    Fancy,
}

struct MyTab {
    kind: MyTabKind,
    /// Just a label for this example. Node identities are opaque handles, not numbers to
    /// show the user, so a tab that wants a number keeps its own.
    number: usize,
}

/// Where a tab asked for from the "+" popup should go, and what it should be.
struct AddRequest {
    path: NodePath,
    kind: MyTabKind,
}

impl MyTab {
    fn regular(number: usize) -> Self {
        Self {
            kind: MyTabKind::Regular,
            number,
        }
    }

    fn fancy(number: usize) -> Self {
        Self {
            kind: MyTabKind::Fancy,
            number,
        }
    }

    fn title(&self) -> String {
        match self.kind {
            MyTabKind::Regular => format!("Regular Tab {}", self.number),
            MyTabKind::Fancy => format!("Fancy Tab {}", self.number),
        }
    }

    fn content(&self) -> RichText {
        match self.kind {
            MyTabKind::Regular => {
                RichText::new(format!("Content of {}. This tab is ho-hum.", self.title()))
            }
            MyTabKind::Fancy => RichText::new(format!(
                "Content of {}. This tab sure is fancy!",
                self.title()
            ))
            .italics()
            .size(20.0)
            .color(Color32::from_rgb(255, 128, 64)),
        }
    }
}

struct TabViewer<'a> {
    added_nodes: &'a mut Vec<AddRequest>,
}

impl egui_dockyard::TabViewer for TabViewer<'_> {
    type Tab = MyTab;

    fn title(&mut self, tab: &Self::Tab) -> egui::Atoms<'static> {
        egui::Atoms::new(tab.title())
    }

    fn ui(&mut self, ui: &mut egui::Ui, tab: &Self::Tab) {
        ui.label(tab.content());
    }

    fn add_popup(&mut self, ui: &mut egui::Ui, path: NodePath) {
        ui.set_min_width(120.0);
        ui.style_mut().visuals.button_frame = false;

        if ui.button("Regular tab").clicked() {
            self.added_nodes.push(AddRequest {
                path,
                kind: MyTabKind::Regular,
            });
        }

        if ui.button("Fancy tab").clicked() {
            self.added_nodes.push(AddRequest {
                path,
                kind: MyTabKind::Fancy,
            });
        }
    }
}

struct MyApp {
    dock_state: DockState<MyTab>,
    counter: usize,
}

impl Default for MyApp {
    fn default() -> Self {
        let mut tree = DockState::new(vec![MyTab::regular(1), MyTab::fancy(2)]);
        let root = tree.main_surface().root().unwrap();

        // You can modify the tree before constructing the dock
        let [a, b] = tree
            .main_surface_mut()
            .split_left(root, 0.3, vec![MyTab::fancy(3)]);
        let [_, _] = tree
            .main_surface_mut()
            .split_below(a, 0.7, vec![MyTab::fancy(4)]);
        let [_, _] = tree
            .main_surface_mut()
            .split_below(b, 0.5, vec![MyTab::regular(5)]);

        Self {
            dock_state: tree,
            counter: 6,
        }
    }
}

impl eframe::App for MyApp {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let mut added_nodes = Vec::new();
        let mut tab_viewer = TabViewer {
            added_nodes: &mut added_nodes,
        };
        DockArea::new(&self.dock_state)
            .show_add_buttons(true)
            .show_add_popup(true)
            .style(Style::from_egui(ui.style().as_ref()))
            .show_inside(ui, &mut tab_viewer)
            .apply(ui.ctx(), &mut self.dock_state);

        added_nodes.drain(..).for_each(|request| {
            self.dock_state.set_focused_node_and_surface(request.path);
            self.dock_state.push_to_focused_leaf(MyTab {
                kind: request.kind,
                number: self.counter,
            });
            self.counter += 1;
        });
    }
}
