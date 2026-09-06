#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")] // hide console window on Windows in release

use eframe::{NativeOptions, egui};
use egui_dockyard::{DockArea, DockState, Style};

fn main() -> eframe::Result<()> {
    let options = NativeOptions::default();
    eframe::run_native(
        "My egui App",
        options,
        Box::new(|_cc| Ok(Box::<MyApp>::default())),
    )
}

#[derive(Default)]
struct TabViewer {
    window_opinions: Vec<(String, bool)>,
}

struct OpinionatedTab {
    can_become_window: Result<bool, bool>,
    title: String,
    content: String,
}

impl egui_dockyard::TabViewer for TabViewer {
    type Tab = OpinionatedTab;

    fn title(&mut self, tab: &Self::Tab) -> egui::Atoms<'static> {
        egui::Atoms::new(tab.title.clone())
    }

    fn ui(&mut self, ui: &mut egui::Ui, tab: &Self::Tab) {
        ui.label(&tab.content);
        match tab.can_become_window {
            Ok(opinion) => {
                let mut next_opinion = opinion;
                if ui
                    .add(egui::Checkbox::new(
                        &mut next_opinion,
                        "can be turned into window",
                    ))
                    .changed()
                {
                    self.window_opinions.push((tab.title.clone(), next_opinion));
                }
            }
            Err(fixed_opinion) => {
                if fixed_opinion {
                    ui.small("this tab can exist in a window");
                } else {
                    ui.small("this tab cannot exist in a window");
                }
            }
        }
    }

    fn allowed_in_windows(&self, tab: &Self::Tab) -> bool {
        match tab.can_become_window {
            Ok(opinion) | Err(opinion) => opinion,
        }
    }
}

struct MyApp {
    tree: DockState<OpinionatedTab>,
}

impl Default for MyApp {
    fn default() -> Self {
        let mut tree = DockState::new(vec![
            OpinionatedTab {
                can_become_window: Ok(false),
                title: "old tab".to_owned(),
                content: "since when could tabs become windows?".to_string(),
            },
            OpinionatedTab {
                can_become_window: Err(false),
                title: "grumpy tab".to_owned(),
                content: "I don't want to be a window!".to_string(),
            },
        ]);

        // You can modify the tree before constructing the dock
        let root = tree.main_surface().root().unwrap();
        let [a, _] = tree.main_surface_mut().split_right(
            root,
            0.6,
            vec![OpinionatedTab {
                can_become_window: Ok(true),
                title: "wise tab".to_owned(),
                content: "egui_dockyard 0.7!".to_string(),
            }],
        );
        let [_, _] = tree.main_surface_mut().split_below(
            a,
            0.4,
            vec![OpinionatedTab {
                can_become_window: Ok(true),
                title: "instructional tab".to_owned(),
                content: "This demo is meant to showcase the ability for tabs to become/be placed inside windows. 
                \nindividual tabs have the ability to accept/reject being put/turned into a window. 
                \nIn this demo some tabs have a fixed opinion on this, others can be swayed with the click of a checkbox. 
                \n\n In your app you yourself may decide how tabs behave, but for now try dragging some tabs into empty space to turn them into windows!"
                .to_string(),
            }],
        );
        let _ = tree.add_window(vec![OpinionatedTab {
            can_become_window: Err(true),
            title: "egotistical tab".to_owned(),
            content: "im above you all!".to_string(),
        }]);

        Self { tree }
    }
}

impl eframe::App for MyApp {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let mut tab_viewer = TabViewer::default();
        DockArea::new(&self.tree)
            .style(Style::from_egui(ui.style().as_ref()))
            .show_inside(ui, &mut tab_viewer)
            .apply(ui.ctx(), &mut self.tree, &mut tab_viewer);

        // The viewer only emits intents while the tree is borrowed for drawing. The owner
        // applies them afterwards, so a tab body never mutates its own dock entry mid-frame.
        for (title, opinion) in tab_viewer.window_opinions {
            for (_, tab) in self.tree.iter_all_tabs_mut() {
                if tab.title == title
                    && let Ok(current) = &mut tab.can_become_window
                {
                    *current = opinion;
                }
            }
        }
    }
}
