#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")] // hide console window on Windows in release

//! Tab titles carrying an icon as well as a name.
//!
//! A title is an [`egui::Atoms`] — a row of text, images, or both — so an icon is a matter of
//! putting one in front of the name rather than of anything the dock has to be configured with.
//! Put first, the icon is also the last part of the title a squeezed tab gives up: drag the split
//! to the left until the names start fading and watch what stays.
//!
//! The icons here are drawn in code into textures, so this example needs no image loaders and no
//! files on disk. A real application would more likely write
//! `Atoms::new((egui::include_image!("../icons/wave.svg"), name))` and install
//! `egui_extras::install_image_loaders` once at startup.

use eframe::{NativeOptions, egui};
use egui::{Atoms, Color32, ColorImage, Image, TextureHandle, TextureOptions, load::SizedTexture};
use egui_dockyard::{DockArea, DockState, Style, TabViewer};

fn main() -> eframe::Result<()> {
    eframe::run_native(
        "Tabs with icons",
        NativeOptions::default(),
        Box::new(|_cc| Ok(Box::<MyApp>::default())),
    )
}

/// One tab: a name and the colour its icon is drawn in.
struct Tab {
    name: String,
    colour: Color32,
}

impl Tab {
    fn new(name: &str, colour: Color32) -> Self {
        Self {
            name: name.to_owned(),
            colour,
        }
    }
}

/// A round dot, `SIZE` square, in `colour`.
///
/// Stands in for whatever an application would really show — a file type, a device that is online,
/// a panel's own emblem. What matters for the dock is only that it is an image.
fn dot(colour: Color32) -> ColorImage {
    const SIZE: usize = 16;
    let centre = (SIZE as f32 - 1.0) / 2.0;
    let mut pixels = Vec::with_capacity(SIZE * SIZE);
    for y in 0..SIZE {
        for x in 0..SIZE {
            let distance = ((x as f32 - centre).powi(2) + (y as f32 - centre).powi(2)).sqrt();
            // A soft edge over the last pixel, so the dot does not come back as a staircase.
            let alpha = (centre - distance).clamp(0.0, 1.0);
            pixels.push(Color32::from_rgba_unmultiplied(
                colour.r(),
                colour.g(),
                colour.b(),
                (alpha * 255.0) as u8,
            ));
        }
    }
    ColorImage::new([SIZE, SIZE], pixels)
}

struct Viewer {
    /// One texture per tab, uploaded once and kept: a title is asked for every frame, and
    /// uploading an image from inside it would be a new texture every frame.
    icons: Vec<TextureHandle>,
}

impl Viewer {
    fn new(ctx: &egui::Context, tabs: &[Tab]) -> Self {
        let icons = tabs
            .iter()
            .map(|tab| ctx.load_texture(&tab.name, dot(tab.colour), TextureOptions::default()))
            .collect();
        Self { icons }
    }

    fn icon_of(&self, tab: &Tab) -> Option<Image<'static>> {
        let handle = self.icons.iter().find(|icon| icon.name() == tab.name)?;
        Some(Image::from_texture(SizedTexture::from_handle(handle)))
    }
}

impl TabViewer for Viewer {
    type Tab = Tab;

    fn title(&mut self, tab: &Self::Tab) -> Atoms<'static> {
        match self.icon_of(tab) {
            // The icon first, the name behind it: that order is what makes the icon the part a
            // squeezed tab keeps.
            Some(icon) => Atoms::new((icon, tab.name.clone())),
            None => Atoms::new(tab.name.clone()),
        }
    }

    fn ui(&mut self, ui: &mut egui::Ui, tab: &Self::Tab) {
        ui.label(format!("Content of {}", tab.name));
        ui.separator();
        ui.label("Narrow this panel — or collapse it sideways with the ⯅ button — and watch the");
        ui.label("icons stay while the names fade out.");
    }
}

struct MyApp {
    tree: DockState<Tab>,
    viewer: Option<Viewer>,
}

impl Default for MyApp {
    fn default() -> Self {
        let mut tree = DockState::new(vec![
            Tab::new("Trajectory", Color32::from_rgb(0x4c, 0x9a, 0xff)),
            Tab::new("Hydraulics", Color32::from_rgb(0x36, 0xb3, 0x7e)),
        ]);

        let root = tree.main_surface().root().unwrap();
        let [left, _] = tree.main_surface_mut().split_left(
            root,
            0.35,
            vec![
                Tab::new("Well log", Color32::from_rgb(0xff, 0xab, 0x00)),
                Tab::new("Casing", Color32::from_rgb(0xd6, 0x4b, 0x6a)),
            ],
        );
        tree.main_surface_mut().split_below(
            left,
            0.6,
            vec![Tab::new("Notes", Color32::from_rgb(0x9b, 0x7d, 0xdb))],
        );

        Self { tree, viewer: None }
    }
}

impl eframe::App for MyApp {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        // Built on the first frame, when there is a context to upload the textures to.
        let viewer = self.viewer.get_or_insert_with(|| {
            let tabs: Vec<Tab> = self
                .tree
                .iter_all_tabs()
                .map(|(_, tab)| Tab::new(&tab.name, tab.colour))
                .collect();
            Viewer::new(ui.ctx(), &tabs)
        });

        DockArea::new(&self.tree)
            .style(Style::from_egui(ui.style().as_ref()))
            .show_leaf_collapse_buttons(true)
            .collapse_sideways(true)
            .show_inside(ui, viewer)
            .apply(ui.ctx(), &mut self.tree, viewer);
    }
}
