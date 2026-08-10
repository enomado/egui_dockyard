/// Container for dockable tabs.
pub mod dock_area;

/// Trait for tab-viewing types.
pub mod tab_viewer;

pub use dock_area::ids::{drag_hover_node, drag_in_flight, dragged_tab, tab_widget_id};
pub use dock_area::{AllowedSplits, DockArea, DragInFlight, DragSource, DragSubject};
pub use tab_viewer::TabViewer;
