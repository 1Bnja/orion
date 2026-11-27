//! «Limpiar» un plan terminado: cierra las pestañas de sus agentes, borra sus worktrees y las
//! ramas ya integradas, y olvida el plan. Nunca borra cambios sin commitear ni trabajo sin fusionar.
//! ponytail: git en el hilo de la ventana; con planes enormes convendría un hilo aparte.

use super::super::{Orion, same_path_key};
use crate::agents;
use eframe::egui;
use std::path::PathBuf;

/// La rama de una subtarea y, si sigue ahí, su worktree.
struct Item {
    branch: String,
    worktree: Option<PathBuf>,
    dirty: bool,
    integrated: bool,
}

/// Lo que haría «Limpiar», calculado antes de tocar nada.
pub(super) struct Cleanup {
    /// La rama actual del repo.
    into: String,
    tabs: usize,
    items: Vec<Item>,
}

/// "1 rama", "2 ramas".
fn count(n: usize, one: &str, many: &str) -> String {
    format!("{n} {}", if n == 1 { one } else { many })
}

impl Cleanup {
    /// El texto de la confirmación.
    pub(super) fn summary(&self) -> String {
        let n = |f: fn(&Item) -> bool| self.items.iter().filter(|i| f(i)).count();
        let worktrees = n(|i| i.worktree.is_some() && !i.dirty);
        let dirty = n(|i| i.dirty);
        let merged = n(|i| !i.dirty && i.integrated);
        let unmerged = n(|i| !i.dirty && !i.integrated);
        let mut text = format!(
            "Se {} {} y se {} {}",
            if self.tabs == 1 { "cierra" } else { "cierran" },
            count(self.tabs, "pestaña", "pestañas"),
            if worktrees == 1 { "borra" } else { "borran" },
            count(worktrees, "worktree", "worktrees"),
        );
        let mut branches = Vec::new();
        if merged > 0 {
            let (are, go) = match merged {
                1 => ("está", "se borra"),
                _ => ("están", "se borran"),
            };
            let what = count(merged, "rama", "ramas");
            branches.push(format!("{what} ya {are} en {} y {go}", self.into));
        }
        if unmerged > 0 {
            let (keep, has) = match unmerged {
                1 => ("se conserva", "tiene"),
                _ => ("se conservan", "tienen"),
            };
            let what = count(unmerged, "rama", "ramas");
            branches.push(format!("{what} {keep} porque {has} trabajo sin fusionar"));
        }
        if !branches.is_empty() {
            text += &format!("; {}", branches.join(", "));
        }
        if dirty > 0 {
            let (stay, has) = match dirty {
                1 => ("se deja", "tiene"),
                _ => ("se dejan", "tienen"),
            };
            let what = count(dirty, "worktree", "worktrees");
            text += &format!("; {what} {stay} (con su rama) porque {has} cambios sin commitear");
        }
        text + ". El plan deja de seguirse."
    }
}

impl Orion {
    /// Las pestañas abiertas en alguno de esos worktrees, de la última a la primera.
    fn tabs_in(&self, items: &[Item]) -> Vec<usize> {
        let keys: Vec<String> = (items.iter())
            .filter_map(|i| i.worktree.as_deref().map(same_path_key))
            .collect();
        let mine = |i: &usize| {
            let worktree = self.tabs[*i].worktree.as_deref();
            worktree.is_some_and(|w| keys.contains(&same_path_key(w)))
        };
        (0..self.tabs.len()).rev().filter(mine).collect()
    }

    /// Calcula lo que haría «Limpiar» y lo deja a la espera de confirmación.
    pub(super) fn ask_clean_plan(&mut self) {
        let o = &self.orchestrate;
        let worktrees = agents::orion_worktrees(&o.dir);
        let items: Vec<Item> = (o.plan.iter())
            .filter_map(|(_, run)| run.branch())
            .map(|branch| {
                let found = worktrees.iter().find(|(_, b)| b == branch);
                let worktree = found.map(|(path, _)| path.clone());
                Item {
                    branch: branch.to_string(),
                    dirty: worktree.as_deref().is_some_and(agents::is_dirty),
                    integrated: agents::integrated(&o.dir, branch),
                    worktree,
                }
            })
            // Sin worktree ni rama no queda nada que limpiar.
            .filter(|i| i.worktree.is_some() || agents::branch_exists(&o.dir, &i.branch))
            .collect();
        let into = agents::current_branch(&o.dir).unwrap_or_else(|_| "HEAD".into());
        let tabs = self.tabs_in(&items).len();
        self.orchestrate.cleanup = Some(Cleanup { into, tabs, items });
    }

    /// Hace lo confirmado y avisa lo que pasó.
    pub(super) fn clean_plan(&mut self, ctx: &egui::Context) {
        let Some(Cleanup { items, .. }) = self.orchestrate.cleanup.take() else {
            return;
        };
        let tabs = self.tabs_in(&items);
        for &i in &tabs {
            self.remove_tab(ctx, i);
        }
        let repo = self.orchestrate.dir.clone();
        let (mut removed, mut deleted, mut kept) = (0, 0, 0);
        let mut problems = String::new();
        for item in &items {
            let branch = &item.branch;
            let result = match &item.worktree {
                Some(path) => {
                    agents::remove_worktree(&repo, path, branch).inspect(|_| removed += 1)
                }
                None => Ok(agents::delete_if_integrated(&repo, branch)),
            };
            match result {
                Ok(true) => deleted += 1,
                Ok(false) => kept += 1,
                Err(_) if item.worktree.as_deref().is_some_and(agents::is_dirty) => {
                    problems += &format!(" · {branch} se dejó: tiene cambios sin commitear");
                }
                Err(e) => {
                    problems += &format!(" · No pude borrar el worktree de {branch}: {e}");
                }
            }
        }
        self.orchestrate.forget();
        let mut text = format!(
            "Plan limpiado. Pestañas cerradas: {}, worktrees borrados: {removed}, ramas borradas: {deleted}",
            tabs.len()
        );
        if kept > 0 {
            text += &format!(", ramas conservadas: {kept}");
        }
        self.show_toast(text + &problems);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn item(dirty: bool, integrated: bool) -> Item {
        Item {
            branch: "orion/a-1".into(),
            worktree: Some(PathBuf::from("wt")),
            dirty,
            integrated,
        }
    }

    #[test]
    fn the_summary_says_what_goes_and_what_stays() {
        let mut cleanup = Cleanup {
            into: "main".into(),
            tabs: 3,
            items: vec![item(false, true), item(false, true), item(false, false)],
        };
        assert_eq!(
            cleanup.summary(),
            "Se cierran 3 pestañas y se borran 3 worktrees; 2 ramas ya están en main y se borran, \
             1 rama se conserva porque tiene trabajo sin fusionar. El plan deja de seguirse."
        );
        // Con cambios sin commitear no se promete borrar ni el worktree ni la rama.
        (cleanup.tabs, cleanup.items) = (1, vec![item(true, true)]);
        assert_eq!(
            cleanup.summary(),
            "Se cierra 1 pestaña y se borran 0 worktrees; 1 worktree se deja (con su rama) porque \
             tiene cambios sin commitear. El plan deja de seguirse."
        );
    }
}
