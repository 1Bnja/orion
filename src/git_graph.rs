//! Datos para la pestaña "Grafo" (`app::graph`): las ramas y commits del repo, en filas
//! con su carril y las líneas que bajan a la fila siguiente (estilo GitKraken, solo lectura).

use std::path::Path;

/// Un commit tal como lo cuenta `git log`.
#[derive(Clone, Debug)]
pub struct Commit {
    pub id: String,
    pub parents: Vec<String>,
    pub refs: Vec<String>,
    pub author: String,
    pub time: i64,
    pub subject: String,
}

/// Una fila del grafo: el carril del commit y las líneas que bajan a la fila siguiente.
#[derive(Clone, Debug)]
pub struct Row {
    pub lane: usize,
    /// Líneas desde los carriles de esta fila hacia los de la fila siguiente.
    pub edges: Vec<(usize, usize)>,
}

/// Los commits y sus filas, en el mismo orden (`rows[i]` corresponde a `commits[i]`).
#[derive(Clone, Debug, Default)]
pub struct Graph {
    pub commits: Vec<Commit>,
    pub rows: Vec<Row>,
    pub lanes: usize,
}

/// Lee hasta `limit` commits de todas las ramas y arma el grafo.
pub fn load(repo: &Path, limit: usize) -> Result<Graph, String> {
    let n = limit.to_string();
    // Un solo comando: cada commit en una línea, con campos separados por 0x1f.
    let text = crate::agents::git(
        repo,
        &[
            "log",
            "--all",
            "--topo-order",
            "-n",
            &n,
            "--format=%H\x1f%P\x1f%D\x1f%an\x1f%at\x1f%s",
        ],
    )?;
    let commits = parse(&text);
    let rows = layout(&commits);

    // El máximo carril usado es el mayor índice que aparece en una fila o en un edge.
    let mut lanes = 0;
    for row in &rows {
        lanes = lanes.max(row.lane + 1);
        for &(a, b) in &row.edges {
            lanes = lanes.max(a + 1).max(b + 1);
        }
    }

    Ok(Graph {
        commits,
        rows,
        lanes,
    })
}

/// Separa el texto de `git log` en commits. Una línea por commit, seis campos por 0x1f:
/// hash, padres, refs, autor, fecha y asunto.
fn parse(text: &str) -> Vec<Commit> {
    text.lines()
        .filter(|line| !line.is_empty())
        .filter_map(|line| {
            let mut fields = line.splitn(6, '\x1f');
            let id = fields.next()?;
            let parents = fields.next()?;
            let refs = fields.next()?;
            let author = fields.next()?;
            let time = fields.next()?;
            let subject = fields.next()?;
            Some(Commit {
                id: id.to_string(),
                parents: parents
                    .split(' ')
                    .filter(|p| !p.is_empty())
                    .map(String::from)
                    .collect(),
                refs: refs
                    .split(", ")
                    .map(str::trim)
                    .filter(|r| !r.is_empty())
                    .map(String::from)
                    .collect(),
                author: author.to_string(),
                time: time.parse().unwrap_or(0),
                subject: subject.to_string(),
            })
        })
        .collect()
}

/// Asigna carriles con el algoritmo clásico: cada carril guarda el commit que espera.
fn layout(commits: &[Commit]) -> Vec<Row> {
    // `lanes[j]` es el id del commit que el carril j está esperando (o None si está libre).
    let mut lanes: Vec<Option<String>> = Vec::new();
    let mut rows: Vec<Row> = Vec::with_capacity(commits.len());

    for commit in commits {
        let before = lanes.clone();

        // Carriles que ya esperaban este commit porque un hijo lo reservó.
        let expecting: Vec<usize> = before
            .iter()
            .enumerate()
            .filter(|(_, slot)| slot.as_deref() == Some(commit.id.as_str()))
            .map(|(i, _)| i)
            .collect();

        // Si varios hijos lo esperaban, nos quedamos con el carril más bajo.
        let lane = match expecting.first() {
            Some(&l) => l,
            None => match lanes.iter().position(|slot| slot.is_none()) {
                Some(j) => j,
                None => {
                    lanes.push(None);
                    lanes.len() - 1
                }
            },
        };

        // Los carriles duplicados quedan libres: convergen al carril elegido.
        // La convergencia se dibuja en la fila anterior, que es la que baja hacia esta.
        if expecting.len() > 1
            && let Some(previous) = rows.last_mut()
        {
            for (_, to) in &mut previous.edges {
                if *to != lane && expecting.contains(to) {
                    *to = lane;
                }
            }
        }
        for &j in &expecting {
            lanes[j] = None;
        }

        // El primer padre hereda el carril; los demás reutilizan uno que los espere o abren hueco.
        let mut parent_lanes = Vec::with_capacity(commit.parents.len());
        if let Some(first) = commit.parents.first() {
            lanes[lane] = Some(first.clone());
            parent_lanes.push(lane);
            for parent in &commit.parents[1..] {
                let j = match lanes
                    .iter()
                    .position(|slot| slot.as_deref() == Some(parent.as_str()))
                {
                    Some(j) => j,
                    None => match lanes.iter().position(|slot| slot.is_none()) {
                        Some(j) => {
                            lanes[j] = Some(parent.clone());
                            j
                        }
                        None => {
                            lanes.push(Some(parent.clone()));
                            lanes.len() - 1
                        }
                    },
                };
                parent_lanes.push(j);
            }
        }
        // Sin padres, el carril del commit queda libre (ya está en None).

        // Líneas de esta fila a la siguiente.
        let mut edges = Vec::new();
        // Los carriles que pasan de largo sin tocarse.
        let width = before.len().max(lanes.len());
        for j in 0..width {
            if j == lane {
                continue;
            }
            let old = before.get(j).and_then(|slot| slot.as_deref());
            let new = lanes.get(j).and_then(|slot| slot.as_deref());
            if old.is_some() && old == new {
                edges.push((j, j));
            }
        }
        // El commit hacia cada padre.
        for &parent_lane in &parent_lanes {
            edges.push((lane, parent_lane));
        }

        rows.push(Row { lane, edges });
    }

    rows
}

#[cfg(test)]
mod tests {
    use super::*;

    fn commit(id: &str, parents: &[&str]) -> Commit {
        Commit {
            id: id.to_string(),
            parents: parents.iter().map(|p| p.to_string()).collect(),
            refs: Vec::new(),
            author: "t".to_string(),
            time: 0,
            subject: id.to_string(),
        }
    }

    #[test]
    fn parse_lee_los_campos_separados_por_0x1f() {
        let text = concat!(
            "abc\x1fdef ghi\x1fHEAD -> main, tag: v1\x1fAna\x1f1700000000\x1farregla\n",
            "sin\x1f\x1f\x1fLuis\x1f1700000001\x1fsolo",
        );
        let commits = parse(text);
        assert_eq!(commits.len(), 2);
        assert_eq!(commits[0].id, "abc");
        assert_eq!(commits[0].parents, vec!["def", "ghi"]);
        assert_eq!(commits[0].refs, vec!["HEAD -> main", "tag: v1"]);
        assert_eq!(commits[0].author, "Ana");
        assert_eq!(commits[0].time, 1_700_000_000);
        assert_eq!(commits[0].subject, "arregla");
        assert!(commits[1].parents.is_empty());
        assert!(commits[1].refs.is_empty());
    }

    #[test]
    fn layout_puro_abre_usa_y_libera_carriles() {
        let commits = vec![
            commit("merge", &["a", "b"]),
            commit("a", &["raiz"]),
            commit("b", &["raiz"]),
            commit("raiz", &[]),
        ];
        let rows = layout(&commits);
        assert_eq!(rows.len(), 4);

        // El merge nace en el carril 0 y reparte a sus dos padres.
        assert_eq!(rows[0].lane, 0);
        assert!(rows[0].edges.contains(&(0, 0)));
        assert!(rows[0].edges.contains(&(0, 1)));

        // La rama lateral ocupa el carril 1 y converge al 0 en su propia fila.
        assert_eq!(rows[1].lane, 0);
        assert_eq!(rows[2].lane, 1);
        assert!(rows[2].edges.contains(&(0, 0)));
        assert!(rows[2].edges.contains(&(1, 0)));

        // La raíz recibe la convergencia y no tiene líneas propias.
        assert_eq!(rows[3].lane, 0);
        assert!(rows[3].edges.is_empty());
        assert!(rows.last().unwrap().edges.is_empty());
    }

    fn run(repo: &Path, args: &[&str]) {
        crate::agents::git(repo, args).unwrap();
    }

    fn add_commit(repo: &Path, file: &str, text: &str, message: &str) {
        std::fs::write(repo.join(file), text).unwrap();
        run(repo, &["add", "."]);
        run(repo, &["commit", "-q", "-m", message]);
    }

    #[test]
    fn load_arma_filas_y_carriles_de_un_merge() {
        let base = std::env::temp_dir().join(format!("orion-graph-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        let repo = base.join("repo");
        std::fs::create_dir_all(&repo).unwrap();

        run(&repo, &["init", "-q"]);
        run(&repo, &["config", "user.name", "t"]);
        run(&repo, &["config", "user.email", "t@t"]);
        add_commit(&repo, "raiz.txt", "raiz\n", "raiz");
        run(&repo, &["branch", "-M", "main"]);

        // Un segundo commit en main y, desde ahí, una rama lateral.
        add_commit(&repo, "base.txt", "base\n", "base");
        run(&repo, &["checkout", "-q", "-b", "lateral"]);
        add_commit(&repo, "lateral.txt", "lado\n", "lateral");
        run(&repo, &["checkout", "-q", "main"]);
        run(&repo, &["merge", "--no-ff", "-q", "-m", "merge", "lateral"]);

        let graph = load(&repo, 100).unwrap();
        assert_eq!(graph.commits.len(), 4);
        assert_eq!(graph.lanes, 2);

        // El merge tiene dos padres, vive en el carril 0 y baja a los carriles 0 y 1.
        let merge = graph
            .commits
            .iter()
            .position(|c| c.parents.len() == 2)
            .unwrap();
        assert_eq!(graph.rows[merge].lane, 0);
        assert!(graph.rows[merge].edges.contains(&(0, 0)));
        assert!(graph.rows[merge].edges.contains(&(0, 1)));

        // La rama lateral ocupa el carril 1 y converge al 0 en su propia fila.
        let lateral = graph
            .commits
            .iter()
            .position(|c| c.subject == "lateral")
            .unwrap();
        assert_eq!(graph.rows[lateral].lane, 1);
        assert!(graph.rows[lateral].edges.contains(&(1, 0)));

        // La raíz recibe la convergencia y no tiene líneas propias.
        let root = graph
            .commits
            .iter()
            .position(|c| c.parents.is_empty())
            .unwrap();
        assert_eq!(graph.rows[root].lane, 0);
        assert!(graph.rows[root].edges.is_empty());
        assert!(graph.rows.last().unwrap().edges.is_empty());

        let _ = std::fs::remove_dir_all(&base);
    }
}
