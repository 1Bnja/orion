//! La geometría del grafo de dependencias (al estilo `git log --graph`): qué líneas van en el
//! margen izquierdo de las tarjetas y en qué carril. No dibuja nada.

use crate::orchestrator::Subtask;

/// Una línea de la tarjeta `from` (la dependencia) a la tarjeta `to` (la que depende).
#[derive(Debug, PartialEq)]
pub(super) struct Edge {
    pub(super) from: usize,
    pub(super) to: usize,
    /// Columna del margen, desde 0.
    pub(super) lane: usize,
}

/// El orden en que se ven las tarjetas: por ronda y, dentro de cada una, por índice.
pub(super) fn screen_order(rounds: &[usize]) -> Vec<usize> {
    let mut order: Vec<usize> = (0..rounds.len()).collect();
    // Estable: conserva el orden de índices dentro de la ronda.
    order.sort_by_key(|&i| rounds[i]);
    order
}

/// Las aristas a dibujar y cuántos carriles ocupan. `order` son los índices de las subtareas de
/// arriba abajo. Dos líneas cuyos tramos se solapan no comparten carril; tocarse en una tarjeta
/// (una cadena) no es solaparse.
pub(super) fn edges(subtasks: &[&Subtask], order: &[usize]) -> (Vec<Edge>, usize) {
    // La fila de pantalla de cada subtarea.
    let mut row = vec![None; subtasks.len()];
    for (y, &i) in order.iter().enumerate() {
        if let Some(slot) = row.get_mut(i) {
            *slot = Some(y);
        }
    }
    // (fila de arriba, fila de abajo, origen, destino); lo que no tiene tarjeta se ignora.
    let mut spans = Vec::new();
    for (to, s) in subtasks.iter().enumerate() {
        for &from in &s.after {
            if let (Some(&Some(a)), Some(&Some(b))) = (row.get(from), row.get(to)) {
                spans.push((a.min(b), a.max(b), from, to));
            }
        }
    }
    spans.sort_unstable();

    // Greedy por fila de inicio: el primer carril libre, o uno nuevo.
    let mut ends: Vec<usize> = Vec::new();
    let edges = spans
        .into_iter()
        .map(|(start, end, from, to)| {
            let lane = match ends.iter().position(|&e| e <= start) {
                Some(lane) => lane,
                None => {
                    ends.push(0);
                    ends.len() - 1
                }
            };
            ends[lane] = end;
            Edge { from, to, lane }
        })
        .collect();
    (edges, ends.len())
}

#[cfg(test)]
mod tests {
    use super::super::view::rounds;
    use super::*;

    /// Las aristas como (origen, destino, carril) y el total de carriles, comprobando de paso que
    /// ningún carril tenga tramos solapados.
    fn graph(after: &[&[usize]]) -> (Vec<(usize, usize, usize)>, usize) {
        let plan: Vec<Subtask> = after
            .iter()
            .map(|a| Subtask {
                title: "t".into(),
                prompt: "p".into(),
                executor: "x".into(),
                after: a.to_vec(),
            })
            .collect();
        let refs: Vec<&Subtask> = plan.iter().collect();
        let order = screen_order(&rounds(&refs));
        let (edges, lanes) = edges(&refs, &order);
        let span = |e: &Edge| {
            let row = |i| order.iter().position(|&j| j == i).unwrap();
            (row(e.from), row(e.to))
        };
        for (n, a) in edges.iter().enumerate() {
            assert!(a.lane < lanes);
            for b in edges[n + 1..].iter().filter(|b| b.lane == a.lane) {
                let ((a0, a1), (b0, b1)) = (span(a), span(b));
                assert!(a1 <= b0 || b1 <= a0, "{a:?} se solapa con {b:?}");
            }
        }
        let edges = edges.iter().map(|e| (e.from, e.to, e.lane)).collect();
        (edges, lanes)
    }

    #[test]
    fn cards_are_ordered_by_round_then_index() {
        assert_eq!(screen_order(&[1, 2, 1, 3, 2]), [0, 2, 1, 4, 3]);
    }

    #[test]
    fn a_linear_chain_fits_in_one_lane() {
        let got = graph(&[&[], &[0], &[1]]);
        assert_eq!(got, (vec![(0, 1, 0), (1, 2, 0)], 1));
    }

    #[test]
    fn two_dependencies_of_one_node_use_two_lanes() {
        let got = graph(&[&[], &[], &[0, 1]]);
        assert_eq!(got, (vec![(0, 2, 0), (1, 2, 1)], 2));
    }

    #[test]
    fn independent_branches_cross_on_screen_and_get_their_own_lane() {
        // En pantalla: 0, 1, 2, 3; las ramas 0→2 y 1→3 quedan intercaladas.
        let got = graph(&[&[], &[], &[0], &[1]]);
        assert_eq!(got, (vec![(0, 2, 0), (1, 3, 1)], 2));
    }

    #[test]
    fn a_freed_lane_is_reused_and_rows_follow_the_screen() {
        // La 2 es de la ronda 1: en pantalla va 0, 2, 1, 3.
        let got = graph(&[&[], &[0], &[], &[1]]);
        assert_eq!(got, (vec![(0, 1, 0), (1, 3, 0)], 1));
    }

    #[test]
    fn without_dependencies_there_are_no_lanes() {
        assert_eq!(graph(&[&[], &[]]), (vec![], 0));
        assert_eq!(graph(&[]), (vec![], 0));
    }

    #[test]
    fn out_of_range_dependencies_are_ignored() {
        assert_eq!(graph(&[&[7], &[0, 9]]), (vec![(0, 1, 0)], 1));
    }
}
