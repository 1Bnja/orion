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

/// Una referencia del repo (rama, tag, stash o worktree) y el commit al que apunta.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct RefItem {
    pub name: String,
    /// Hash del commit.
    pub target: String,
    pub current: bool,
}

/// Todas las referencias del repo, agrupadas por tipo.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Refs {
    pub local: Vec<RefItem>,
    pub remote: Vec<RefItem>,
    pub tags: Vec<RefItem>,
    pub stashes: Vec<RefItem>,
    pub worktrees: Vec<RefItem>,
}

/// Un archivo cambiado y sus líneas; `None` en binarios y en archivos no rastreados.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct FileChange {
    pub path: String,
    pub added: Option<u32>,
    pub removed: Option<u32>,
}

/// El detalle de un commit.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Details {
    /// Mensaje completo.
    pub message: String,
    pub files: Vec<FileChange>,
}

/// Lee hasta `limit` commits y arma el grafo: de todas las ramas, o con `rev` solo los
/// alcanzables desde ahí.
pub fn load_rev(repo: &Path, rev: Option<&str>, limit: usize) -> Result<Graph, String> {
    let n = limit.to_string();
    // Un solo comando: cada commit en una línea, con campos separados por 0x1f.
    let mut args = vec![
        "log",
        "--topo-order",
        "-n",
        &n,
        "--format=%H\x1f%P\x1f%D\x1f%an\x1f%at\x1f%s",
    ];
    match rev {
        // `--end-of-options` evita que un rev que empiece con `-` se lea como opción.
        Some(rev) => args.extend(["--end-of-options", rev]),
        None => args.push("--all"),
    }
    args.push("--");
    let text = crate::agents::git(repo, &args)?;
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

/// Ramas locales y remotas, tags, stashes y worktrees del repo.
pub fn refs(repo: &Path) -> Result<Refs, String> {
    let mut refs = Refs::default();

    let text = crate::agents::git(
        repo,
        &[
            "for-each-ref",
            // `*objectname` solo existe en tags anotados: es el commit al que apuntan.
            "--format=%(HEAD)%1f%(objectname)%1f%(*objectname)%1f%(refname)",
            "refs/heads",
            "refs/remotes",
            "refs/tags",
        ],
    )?;
    for line in text.lines() {
        // El primer campo es `*` o un espacio (que `git` puede haber recortado).
        let mut fields = line.split('\x1f');
        let (Some(head), Some(object), Some(peeled), Some(full)) =
            (fields.next(), fields.next(), fields.next(), fields.next())
        else {
            continue;
        };
        let (list, name) = if let Some(name) = full.strip_prefix("refs/heads/") {
            (&mut refs.local, name)
        } else if let Some(name) = full.strip_prefix("refs/remotes/") {
            if name.ends_with("/HEAD") {
                continue;
            }
            (&mut refs.remote, name)
        } else if let Some(name) = full.strip_prefix("refs/tags/") {
            (&mut refs.tags, name)
        } else {
            continue;
        };
        list.push(RefItem {
            name: name.to_string(),
            target: if peeled.is_empty() { object } else { peeled }.to_string(),
            current: head == "*",
        });
    }

    let text = crate::agents::git(repo, &["stash", "list", "--format=%H%x1f%gd: %gs"])?;
    refs.stashes = text
        .lines()
        .filter_map(|line| line.split_once('\x1f'))
        .map(|(target, name)| RefItem {
            name: name.to_string(),
            target: target.to_string(),
            current: false,
        })
        .collect();

    // Se comparan rutas canónicas: git y Windows no siempre las escriben igual.
    let here = crate::agents::repo_root(repo)
        .ok()
        .and_then(|root| root.canonicalize().ok());
    let text = crate::agents::git(repo, &["worktree", "list", "--porcelain"])?;
    let mut path = "";
    for line in text.lines() {
        if let Some(p) = line.strip_prefix("worktree ") {
            path = p;
        } else if let Some(head) = line.strip_prefix("HEAD ") {
            // Un repo bare no trae línea HEAD y queda fuera.
            refs.worktrees.push(RefItem {
                name: path.to_string(),
                target: head.to_string(),
                current: here.is_some() && Path::new(path).canonicalize().ok() == here,
            });
        }
    }

    Ok(refs)
}

/// Mensaje completo y archivos cambiados de un commit. Un merge puede venir sin archivos.
pub fn details(repo: &Path, id: &str) -> Result<Details, String> {
    // El NUL tras `%B` separa el mensaje del numstat.
    let text = crate::agents::git(
        repo,
        &[
            "show",
            "--numstat",
            "--no-renames",
            "-z",
            "--format=%B%x00",
            "--end-of-options",
            id,
            "--",
        ],
    )?;
    let (message, numstat) = text.split_once('\0').unwrap_or((&text, ""));
    Ok(Details {
        message: message.trim_end().to_string(),
        files: parse_numstat(numstat),
    })
}

/// El diff de un archivo en un commit. `path` es relativo a la raíz del repo.
/// ponytail: `agents::git` recorta la salida, así que se pierden las líneas en blanco finales;
/// si la UI las necesita, habrá que leer la salida cruda.
pub fn diff(repo: &Path, id: &str, path: &str) -> Result<String, String> {
    let path = format!(":(top,literal){path}");
    crate::agents::git(
        repo,
        &[
            "show",
            "--format=",
            "--no-renames",
            "--no-color",
            "--no-ext-diff",
            "--end-of-options",
            id,
            "--",
            &path,
        ],
    )
}

/// Cambios sin commitear (staged, unstaged y no rastreados). Vacío = limpio.
pub fn wip(repo: &Path) -> Result<Vec<FileChange>, String> {
    // Sin commits todavía no hay HEAD: se compara contra el árbol vacío.
    // ponytail: es el hash SHA-1; un repo SHA-256 sin commits daría error.
    let base = match crate::agents::git(repo, &["rev-parse", "--verify", "-q", "HEAD"]) {
        Ok(_) => "HEAD",
        Err(_) => "4b825dc642cb6eb9a060e54bf8d69288fbee4904",
    };
    let tracked = crate::agents::git(
        repo,
        &["diff", base, "--numstat", "--no-renames", "-z", "--"],
    )?;
    let untracked = crate::agents::git(
        repo,
        &[
            "ls-files",
            "--others",
            "--exclude-standard",
            "--full-name",
            "-z",
            "--",
            ":/",
        ],
    )?;
    let mut files = parse_numstat(&tracked);
    files.extend(
        untracked
            .split('\0')
            .filter(|path| !path.is_empty())
            .map(|path| FileChange {
                path: path.to_string(),
                added: None,
                removed: None,
            }),
    );
    Ok(files)
}

/// Un texto que cambia si cambian HEAD, alguna ref o el WIP; sirve para saber cuándo recargar.
pub fn fingerprint(repo: &Path) -> Result<String, String> {
    // Falla en un repo sin commits, y ahí vacío es una respuesta válida.
    let head =
        crate::agents::git(repo, &["rev-parse", "--verify", "-q", "HEAD"]).unwrap_or_default();
    // `%(HEAD)` delata un cambio de rama aunque ambas apunten al mismo commit.
    let refs = crate::agents::git(
        repo,
        &["for-each-ref", "--format=%(HEAD)%(objectname)%(refname)"],
    )?;
    // Sin locks opcionales, para no chocar con los git de los agentes al consultar seguido.
    let status = crate::agents::git(repo, &["--no-optional-locks", "status", "--porcelain"])?;
    let mut print = format!("{head}\n{refs}\n{status}");

    // `status` no cambia al volver a editar un archivo ya modificado: se suman tamaño y fecha.
    let root = crate::agents::repo_root(repo)?;
    let dirty = crate::agents::git(
        repo,
        &[
            "ls-files",
            "--modified",
            "--others",
            "--exclude-standard",
            "--full-name",
            "-z",
            "--",
            ":/",
        ],
    )?;
    for path in dirty.split('\0').filter(|path| !path.is_empty()) {
        if let Ok(meta) = std::fs::metadata(root.join(path)) {
            print += &format!("\n{path} {} {:?}", meta.len(), meta.modified().ok());
        }
    }
    Ok(print)
}

/// Separa la salida de `--numstat -z --no-renames`: `agregadas\tquitadas\truta` y un NUL.
fn parse_numstat(text: &str) -> Vec<FileChange> {
    text.split('\0')
        .filter_map(|record| {
            let mut fields = record.trim_start_matches('\n').splitn(3, '\t');
            let added = fields.next()?;
            let removed = fields.next()?;
            let path = fields.next()?;
            Some(FileChange {
                path: path.to_string(),
                // En binarios numstat pone `-`, que no parsea: queda `None`.
                added: added.parse().ok(),
                removed: removed.parse().ok(),
            })
        })
        .collect()
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

        let graph = load_rev(&repo, None, 100).unwrap();
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

    /// Crea un repo temporal con un commit en `main`; devuelve la carpeta base y el repo.
    fn temp_repo(name: &str) -> (std::path::PathBuf, std::path::PathBuf) {
        let base = std::env::temp_dir().join(format!("orion-graph-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        let repo = base.join("repo");
        std::fs::create_dir_all(&repo).unwrap();
        run(&repo, &["init", "-q"]);
        run(&repo, &["config", "user.name", "t"]);
        run(&repo, &["config", "user.email", "t@t"]);
        add_commit(&repo, "raiz.txt", "raiz\n", "raiz");
        run(&repo, &["branch", "-M", "main"]);
        (base, repo)
    }

    fn head(repo: &Path) -> String {
        crate::agents::git(repo, &["rev-parse", "HEAD"]).unwrap()
    }

    #[test]
    fn load_rev_de_una_rama_no_trae_commits_de_la_otra() {
        let (base, repo) = temp_repo("rev");
        run(&repo, &["checkout", "-q", "-b", "lateral"]);
        add_commit(&repo, "lateral.txt", "lado\n", "lateral");
        run(&repo, &["checkout", "-q", "main"]);
        add_commit(&repo, "main.txt", "main\n", "solo-main");

        let subjects = |rev| -> Vec<String> {
            let graph = load_rev(&repo, rev, 100).unwrap();
            graph.commits.into_iter().map(|c| c.subject).collect()
        };
        assert_eq!(subjects(Some("main")), vec!["solo-main", "raiz"]);
        assert_eq!(subjects(Some("lateral")), vec!["lateral", "raiz"]);
        assert_eq!(subjects(None).len(), 3);

        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn refs_agrupa_ramas_tags_stashes_y_worktrees() {
        let (base, repo) = temp_repo("refs");
        let first = head(&repo);

        let empty = refs(&repo).unwrap();
        assert!(empty.remote.is_empty());
        assert!(empty.tags.is_empty());
        assert!(empty.stashes.is_empty());

        run(&repo, &["tag", "ligero"]);
        run(&repo, &["tag", "-a", "-m", "v1", "anotado"]);
        run(&repo, &["update-ref", "refs/remotes/origin/main", "HEAD"]);
        let origin_head = "refs/remotes/origin/HEAD";
        run(
            &repo,
            &["symbolic-ref", origin_head, "refs/remotes/origin/main"],
        );
        let other = base.join("otro");
        let other_arg = other.to_str().unwrap();
        run(&repo, &["worktree", "add", "-q", "-b", "otra", other_arg]);
        std::fs::write(repo.join("raiz.txt"), "cambio\n").unwrap();
        run(&repo, &["stash", "push", "-q", "-m", "guardado"]);

        let all = refs(&repo).unwrap();
        let item = |name: &str, current| RefItem {
            name: name.to_string(),
            target: first.clone(),
            current,
        };
        assert_eq!(all.local, vec![item("main", true), item("otra", false)]);
        assert_eq!(all.remote, vec![item("origin/main", false)]);
        assert_eq!(
            all.tags,
            vec![item("anotado", false), item("ligero", false)]
        );
        assert_eq!(all.stashes.len(), 1);
        assert!(all.stashes[0].name.starts_with("stash@{0}: "));
        assert!(all.stashes[0].name.ends_with("guardado"));
        assert_eq!(all.worktrees.len(), 2);
        assert!(all.worktrees[0].current);
        assert!(!all.worktrees[1].current);
        assert!(all.worktrees[1].name.ends_with("otro"));
        assert_eq!(all.worktrees[1].target, first);

        // Visto desde el otro worktree, el actual es ese.
        let from_other = refs(&other).unwrap();
        assert!(!from_other.worktrees[0].current);
        assert!(from_other.worktrees[1].current);

        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn details_trae_mensaje_completo_y_numstat() {
        let (base, repo) = temp_repo("details");
        std::fs::write(repo.join("año.bin"), [0u8, 1, 2]).unwrap();
        add_commit(&repo, "raiz.txt", "uno\ndos\n", "asunto\n\ncuerpo");

        let info = details(&repo, &head(&repo)).unwrap();
        assert_eq!(info.message, "asunto\n\ncuerpo");
        let binary = FileChange {
            path: "año.bin".to_string(),
            added: None,
            removed: None,
        };
        let text = FileChange {
            path: "raiz.txt".to_string(),
            added: Some(2),
            removed: Some(1),
        };
        assert_eq!(info.files, vec![binary, text]);

        // Un merge no falla.
        run(&repo, &["checkout", "-q", "-b", "lateral"]);
        add_commit(&repo, "lateral.txt", "lado\n", "lateral");
        run(&repo, &["checkout", "-q", "main"]);
        run(&repo, &["merge", "--no-ff", "-q", "-m", "merge", "lateral"]);
        assert_eq!(details(&repo, &head(&repo)).unwrap().message, "merge");

        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn diff_funciona_en_el_primer_commit_y_en_los_siguientes() {
        let (base, repo) = temp_repo("diff");
        let first = head(&repo);
        add_commit(&repo, "raiz.txt", "nueva\n", "cambio");

        let initial = diff(&repo, &first, "raiz.txt").unwrap();
        assert!(initial.contains("+raiz"));
        let next = diff(&repo, &head(&repo), "raiz.txt").unwrap();
        assert!(next.contains("-raiz"));
        assert!(next.contains("+nueva"));

        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn wip_lista_staged_unstaged_y_no_rastreados() {
        let (base, repo) = temp_repo("wip");
        assert!(wip(&repo).unwrap().is_empty());

        std::fs::write(repo.join("raiz.txt"), "raiz\nmas\n").unwrap();
        std::fs::write(repo.join("staged.txt"), "a\n").unwrap();
        run(&repo, &["add", "staged.txt"]);
        std::fs::write(repo.join("suelto.txt"), "b\n").unwrap();

        let change = |path: &str, added, removed| FileChange {
            path: path.to_string(),
            added,
            removed,
        };
        assert_eq!(
            wip(&repo).unwrap(),
            vec![
                change("raiz.txt", Some(1), Some(0)),
                change("staged.txt", Some(1), Some(0)),
                change("suelto.txt", None, None),
            ]
        );

        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn fingerprint_cambia_con_commits_ramas_y_ediciones() {
        let (base, repo) = temp_repo("fingerprint");
        let mut seen = vec![fingerprint(&repo).unwrap()];
        assert_eq!(fingerprint(&repo).unwrap(), seen[0]);
        let mut changed = || {
            let now = fingerprint(&repo).unwrap();
            assert!(!seen.contains(&now));
            seen.push(now);
        };

        add_commit(&repo, "otro.txt", "otro\n", "otro");
        changed();
        run(&repo, &["branch", "nueva"]);
        changed();
        run(&repo, &["checkout", "-q", "nueva"]);
        changed();
        std::fs::write(repo.join("raiz.txt"), "editado\n").unwrap();
        changed();
        // Otra edición del mismo archivo, que `status` por sí solo no notaría.
        std::fs::write(repo.join("raiz.txt"), "editado otra vez\n").unwrap();
        changed();

        let _ = std::fs::remove_dir_all(&base);
    }
}
