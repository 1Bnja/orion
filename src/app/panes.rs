//! Splits: una pestaña se divide en paneles, cada uno con su propia shell.
//!
//! Es un árbol. Cada `Split` reparte su espacio en partes iguales entre sus hijos, en fila
//! (lado a lado) o en columna (uno arriba del otro); las hojas son los paneles.
//! Es genérico sobre la hoja para poder probarlo sin lanzar shells.

use eframe::egui;

/// Lo que va en cada hoja: solo necesitamos poder identificarla.
pub(super) trait Leaf {
    fn id(&self) -> u64;
}

pub(super) enum Pane<T> {
    Leaf(T),
    Split(Split<T>),
}

pub(super) struct Split<T> {
    /// true = los hijos van lado a lado; false = uno arriba del otro.
    row: bool,
    children: Vec<Pane<T>>,
}

impl<T: Leaf> Split<T> {
    pub(super) fn new(first: T) -> Self {
        Self {
            row: true,
            children: vec![Pane::Leaf(first)],
        }
    }

    /// Divide el panel `id`: `new` queda a su derecha (`row`) o debajo.
    /// Si en esa dirección ya hay hermanos, se suma como uno más; si no, el panel pasa a ser un split.
    /// Si `id` no está, devuelve `new`.
    pub(super) fn split(&mut self, id: u64, row: bool, mut new: T) -> Result<(), T> {
        for i in 0..self.children.len() {
            let hit = matches!(&self.children[i], Pane::Leaf(l) if l.id() == id);
            if hit {
                // Un split con un solo hijo (la raíz sin divisiones) puede cambiar de dirección.
                if self.row == row || self.children.len() == 1 {
                    self.row = row;
                    self.children.insert(i + 1, Pane::Leaf(new));
                } else {
                    let old = self.children.remove(i);
                    let children = vec![old, Pane::Leaf(new)];
                    self.children
                        .insert(i, Pane::Split(Split { row, children }));
                }
                return Ok(());
            }
            if let Pane::Split(sub) = &mut self.children[i] {
                match sub.split(id, row, new) {
                    Ok(()) => return Ok(()),
                    Err(back) => new = back,
                }
            }
        }
        Err(new)
    }

    /// Saca el panel `id`. Un split que queda con un solo hijo se reemplaza por ese hijo.
    pub(super) fn remove(&mut self, id: u64) -> Option<T> {
        let found = self
            .children
            .iter()
            .position(|c| matches!(c, Pane::Leaf(l) if l.id() == id));
        let leaf = match found {
            Some(i) => match self.children.remove(i) {
                Pane::Leaf(leaf) => leaf,
                Pane::Split(_) => unreachable!(),
            },
            None => self.children.iter_mut().find_map(|child| {
                let Pane::Split(sub) = &mut *child else {
                    return None;
                };
                let leaf = sub.remove(id)?;
                if sub.children.len() == 1 {
                    *child = sub.children.pop().unwrap();
                }
                Some(leaf)
            })?,
        };
        // Si quedó un único hijo que es un split, este nivel sobra: se reemplaza por él.
        if let [Pane::Split(_)] = self.children.as_slice()
            && let Some(Pane::Split(only)) = self.children.pop()
        {
            *self = only;
        }
        Some(leaf)
    }

    /// Todos los paneles, de izquierda a derecha y de arriba abajo.
    pub(super) fn leaves(&self) -> Vec<&T> {
        let mut out = Vec::new();
        for child in &self.children {
            match child {
                Pane::Leaf(leaf) => out.push(leaf),
                Pane::Split(sub) => out.extend(sub.leaves()),
            }
        }
        out
    }

    pub(super) fn leaves_mut(&mut self) -> Vec<&mut T> {
        let mut out = Vec::new();
        for child in &mut self.children {
            match child {
                Pane::Leaf(leaf) => out.push(leaf),
                Pane::Split(sub) => out.extend(sub.leaves_mut()),
            }
        }
        out
    }

    pub(super) fn get(&self, id: u64) -> Option<&T> {
        self.leaves().into_iter().find(|l| l.id() == id)
    }

    pub(super) fn get_mut(&mut self, id: u64) -> Option<&mut T> {
        self.leaves_mut().into_iter().find(|l| l.id() == id)
    }

    /// Dónde va cada panel dentro de `rect`, con `gap` píxeles entre hermanos.
    /// Los bordes se redondean a píxeles enteros: las fuentes pixeladas no se ven borrosas.
    pub(super) fn layout(&self, rect: egui::Rect, gap: f32, out: &mut Vec<(u64, egui::Rect)>) {
        let n = self.children.len() as f32;
        let (start, total) = if self.row {
            (rect.min.x, rect.width())
        } else {
            (rect.min.y, rect.height())
        };
        let size = (total - gap * (n - 1.0)) / n;
        for (i, child) in self.children.iter().enumerate() {
            let a = (start + i as f32 * (size + gap)).round();
            let b = (start + i as f32 * (size + gap) + size).round();
            let r = if self.row {
                egui::Rect::from_x_y_ranges(a..=b, rect.y_range())
            } else {
                egui::Rect::from_x_y_ranges(rect.x_range(), a..=b)
            };
            match child {
                Pane::Leaf(leaf) => out.push((leaf.id(), r)),
                Pane::Split(sub) => sub.layout(r, gap, out),
            }
        }
    }
}

/// El panel más cercano a `from` en la dirección `dir` (ej. (1, 0) = a la derecha), según `rects`.
/// Solo cuentan los que están completamente de ese lado y se solapan en el otro eje.
pub(super) fn neighbor(rects: &[(u64, egui::Rect)], from: u64, dir: egui::Vec2) -> Option<u64> {
    let (_, cur) = rects.iter().find(|(id, _)| *id == from)?;
    let beside = |r: &egui::Rect| {
        let overlap_x = r.min.x < cur.max.x && r.max.x > cur.min.x;
        let overlap_y = r.min.y < cur.max.y && r.max.y > cur.min.y;
        // 0.5 de tolerancia por el redondeo de los bordes.
        (dir.x > 0.0 && r.min.x >= cur.max.x - 0.5 && overlap_y)
            || (dir.x < 0.0 && r.max.x <= cur.min.x + 0.5 && overlap_y)
            || (dir.y > 0.0 && r.min.y >= cur.max.y - 0.5 && overlap_x)
            || (dir.y < 0.0 && r.max.y <= cur.min.y + 0.5 && overlap_x)
    };
    let distance = |r: &egui::Rect| (r.center() - cur.center()).length_sq();
    rects
        .iter()
        .filter(|(id, r)| *id != from && beside(r))
        .min_by(|a, b| distance(&a.1).total_cmp(&distance(&b.1)))
        .map(|(id, _)| *id)
}

#[cfg(test)]
mod tests {
    use super::*;

    impl Leaf for u64 {
        fn id(&self) -> u64 {
            *self
        }
    }

    fn ids(split: &Split<u64>) -> Vec<u64> {
        split.leaves().into_iter().copied().collect()
    }

    fn rects(split: &Split<u64>) -> Vec<(u64, egui::Rect)> {
        let mut out = Vec::new();
        let screen = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(100.0, 100.0));
        split.layout(screen, 0.0, &mut out);
        out
    }

    #[test]
    fn splitting_and_closing_panes() {
        let mut tree = Split::new(0);
        tree.split(0, true, 1).unwrap(); // 0 | 1
        tree.split(1, false, 2).unwrap(); // 0 | (1 / 2)
        tree.split(0, true, 3).unwrap(); // 0 | 3 | (1 / 2): misma dirección, se suma al lado
        assert_eq!(ids(&tree), [0, 3, 1, 2]);
        assert_eq!(tree.split(9, true, 4), Err(4)); // no existe

        let r = rects(&tree);
        let at = |id| r.iter().find(|(i, _)| *i == id).unwrap().1;
        assert_eq!(at(0).width(), 33.0);
        assert_eq!(at(1).height(), 50.0); // 1 y 2 se reparten la columna

        // Cerrar 2: el split (1 / 2) queda con un hijo y 1 ocupa la columna entera.
        assert_eq!(tree.remove(2), Some(2));
        assert_eq!(
            rects(&tree)
                .iter()
                .find(|(i, _)| *i == 1)
                .unwrap()
                .1
                .height(),
            100.0
        );
        assert_eq!(tree.remove(0), Some(0));
        assert_eq!(tree.remove(3), Some(3));
        assert_eq!(ids(&tree), [1]);
        assert_eq!(tree.remove(1), Some(1));
        assert!(tree.leaves().is_empty());
    }

    #[test]
    fn root_with_one_split_child_is_lifted() {
        let mut tree = Split::new(0);
        tree.split(0, false, 1).unwrap(); // 0 / 1 (la raíz cambia a columna)
        tree.split(1, true, 2).unwrap(); // 0 / (1 | 2)
        tree.remove(0); // queda solo (1 | 2): ahora es la raíz
        let r = rects(&tree);
        assert_eq!(r[0].1.height(), 100.0);
        assert_eq!(r[1].1.min.x, 50.0);
    }

    #[test]
    fn moving_focus_between_panes() {
        let mut tree = Split::new(0);
        tree.split(0, true, 1).unwrap();
        tree.split(1, false, 2).unwrap(); // 0 | (1 / 2)
        let r = rects(&tree);
        let right = egui::vec2(1.0, 0.0);
        assert_eq!(neighbor(&r, 0, right), Some(1)); // empatan en distancia: el primero
        assert_eq!(neighbor(&r, 2, -right), Some(0));
        assert_eq!(neighbor(&r, 1, egui::vec2(0.0, 1.0)), Some(2));
        assert_eq!(neighbor(&r, 0, egui::vec2(0.0, -1.0)), None); // no hay nada arriba
    }
}
