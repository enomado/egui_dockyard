#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")] // hide console window on Windows in release

use eframe::{NativeOptions, egui};
use egui_dockyard::tab_viewer::OnCloseResponse;
use egui_dockyard::{DockArea, DockState, Style};

fn main() -> eframe::Result<()> {
    let options = NativeOptions::default();
    eframe::run_native(
        "My egui App",
        options,
        Box::new(|_cc| Ok(Box::<MyApp>::default())),
    )
}

struct TabViewer {}

impl egui_dockyard::TabViewer for TabViewer {
    type Tab = String;

    fn title(&mut self, tab: &Self::Tab) -> egui::Atoms<'static> {
        egui::Atoms::new(tab.clone())
    }

    fn ui(&mut self, ui: &mut egui::Ui, tab: &Self::Tab) {
        ui.label(format!("Content of {tab}"));
    }

    fn on_close(&mut self, _tab: &Self::Tab) -> OnCloseResponse {
        println!("Closed tab: {_tab}");
        OnCloseResponse::Close
    }
}

struct MyApp {
    tree: DockState<String>,
}

impl Default for MyApp {
    fn default() -> Self {
        let mut tree = DockState::new(vec!["tab1".to_owned(), "tab2".to_owned()]);

        // You can modify the tree before constructing the dock
        let root = tree.main_surface().root().unwrap();
        let [a, b] = tree
            .main_surface_mut()
            .split_left(root, 0.3, vec!["tab3".to_owned()]);
        let [_, _] = tree
            .main_surface_mut()
            .split_below(a, 0.7, vec!["tab4".to_owned()]);
        let [_, _] = tree
            .main_surface_mut()
            .split_below(b, 0.5, vec!["tab5".to_owned()]);

        Self { tree }
    }
}

impl eframe::App for MyApp {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        DockArea::new(&mut self.tree)
            .style(Style::from_egui(ui.style().as_ref()))
            .show_inside(ui, &mut TabViewer {});
    }
}
