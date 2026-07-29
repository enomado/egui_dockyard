//! Текстовый дамп формы дока — то, что имеет смысл сравнивать до и после round-trip'а.
//!
//! # Почему это здесь, а не у каждого звателя
//!
//! Один и тот же дамп понадобился трижды: гейтам корпуса раскладок в приложении, гейту
//! формата без приложения (`dock_layout_gate`) и — в своём варианте — DST-симулятору. Два
//! первых сравнивают ОДНО И ТО ЖЕ свойство (пережила ли форма запись и чтение), и когда
//! правило выводится заново в каждом месте вызова, копии разъезжаются молча: ровно так на
//! P3 разъехались две метлы поверхностей.
//!
//! DST-трейс намеренно НЕ сведён сюда: он решает другую задачу — воспроизводимость прогона
//! по сиду, — поэтому в нём нет ни долей сплитов, ни счётчиков свёрнутого, зато есть
//! активный таб каждого листа. Это не «ещё одна копия», а другое утверждение.
//!
//! # Что в дампе есть и почему
//!
//! * Поверхности нумеруются позицией в СОХРАНЁННОЙ форме (main = 0, окно N → N+1): дамп
//!   сравнивается с дампом того же дока после round-trip'а, а в памяти main живёт отдельным
//!   полем и окна считаются с нуля (P6). Без этого перевода дамп ловил бы собственную
//!   адресацию, а не формат.
//! * Узлы нумеруются обходом в ширину, а НЕ идентичностями: у дока до и после сохранения
//!   идентичности заведомо разные — они не переживают файл и не должны.
//! * Доли сплитов и счётчики свёрнутого входят в дамп: писатель, теряющий `fraction`,
//!   обязан быть виден (на P2 такая мутация роняла 8 из 13 раскладок).

use std::fmt::Write;

use crate::core::DockState;
use crate::core::surface_index::SurfaceIndex;
use crate::core::tree::node::Node;
use crate::core::tree::node_id::NodeId;

/// Позиция поверхности в сохранённой форме: main всегда 0, окно `n` — на `n + 1`.
fn stored_position(index: SurfaceIndex) -> usize {
    match index {
        SurfaceIndex::Main => 0,
        SurfaceIndex::Window(window) => window.0 + 1,
    }
}

/// Форма дока в виде текста: поверхности, узлы, доли сплитов, состав и порядок табов.
///
/// `tab_label` называет таб так, как удобно звателю (`Debug`, заголовок, имя варианта) —
/// сама форма от этого не зависит, но расхождение состава табов должно быть читаемым.
///
/// Дамп предназначен для СРАВНЕНИЯ (до/после сохранения), а не для разбора: его формат
/// может меняться, лишь бы менялся одинаково для обеих сторон сравнения.
pub fn dock_shape<Tab>(state: &DockState<Tab>, tab_label: impl Fn(&Tab) -> String) -> String {
    let mut out = String::new();
    for (surface_index, surface) in state.iter_surfaces_indexed() {
        let Some(tree) = surface.node_tree() else {
            writeln!(out, "surface {}: empty", stored_position(surface_index)).unwrap();
            continue;
        };
        let order = tree.breadth_first();
        let position = |id: NodeId| order.iter().position(|other| *other == id);
        writeln!(
            out,
            "surface {}: {} узлов, фокус {:?}",
            stored_position(surface_index),
            order.len(),
            tree.focused_leaf().and_then(position)
        )
        .unwrap();
        for (index, id) in order.iter().enumerate() {
            match &tree[*id] {
                Node::Leaf(leaf) => {
                    let tabs: Vec<String> = leaf.iter_tabs().map(&tab_label).collect();
                    writeln!(
                        out,
                        "  {index}: leaf active={:?} collapsed={} tabs={tabs:?}",
                        leaf.active_index().map(|i| i.0),
                        leaf.collapsed
                    )
                    .unwrap();
                }
                node @ (Node::Horizontal(split) | Node::Vertical(split)) => {
                    writeln!(
                        out,
                        "  {index}: {} fraction={} collapsed={} дети={:?}",
                        if node.is_vertical() {
                            "vertical"
                        } else {
                            "horizontal"
                        },
                        split.fraction,
                        split.fully_collapsed,
                        split.children().map(position),
                    )
                    .unwrap();
                }
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::tree::Split;
    use crate::core::tree::node::Node;

    /// Дамп обязан РАЗЛИЧАТЬ доки, отличающиеся только тем, что он берётся судить.
    ///
    /// Зуб на «различие даром»: если бы дамп печатал одну лишь форму дерева, две раскладки
    /// с разной долей сплита читались бы одинаково, и гейт round-trip'а молча перестал бы
    /// ловить потерю `fraction`.
    #[test]
    fn the_shape_notices_a_fraction_a_focus_and_a_tab_order() {
        let mut state = DockState::new(vec!["a".to_string()]);
        let root = state.main_surface().root().unwrap();
        let [left, _right] =
            state
                .main_surface_mut()
                .split(root, Split::Right, 0.5, Node::leaf("b".to_string()));
        let base = dock_shape(&state, |tab| tab.clone());

        // Доля сплита.
        let mut moved = state.clone();
        let split_id = moved.main_surface().root().unwrap();
        match &mut moved.main_surface_mut()[split_id] {
            Node::Horizontal(split) | Node::Vertical(split) => split.fraction = 0.25,
            Node::Leaf(_) => unreachable!("корень сцены — сплит по построению"),
        }
        assert_ne!(
            base,
            dock_shape(&moved, |tab| tab.clone()),
            "дамп не заметил изменившуюся долю сплита"
        );

        // Фокус.
        let mut focused = state.clone();
        focused.main_surface_mut().set_focused_node(left);
        assert_ne!(
            base,
            dock_shape(&focused, |tab| tab.clone()),
            "дамп не заметил переехавший фокус"
        );

        // Состав табов.
        let mut extra = state.clone();
        extra
            .main_surface_mut()
            .leaf_mut(left)
            .unwrap()
            .append_tab("c".to_string());
        assert_ne!(
            base,
            dock_shape(&extra, |tab| tab.clone()),
            "дамп не заметил добавленный таб"
        );
    }
}
