<p align="center">
  <img src="assets/orion.png" width="128" alt="Orion icon">
</p>

<h1 align="center">Orion</h1>

<p align="center">A simple, fast and quiet terminal for Windows and macOS.</p>

---

Orion keeps your terminal work organized without asking you to learn anything new. You use your
usual shell, and every command lands in its own **block**, with its result and how long it took
in plain sight. It's also built for working with coding agents: run several in parallel, each on
its own branch, and Orion tells you when one of them needs you.

## Features

### Command blocks

- Every command and its output form a block, with a label such as `✓ 120 ms` or `✗ 3 · 1.2 s`.
- A failed command stands out in red, so errors are easy to spot.
- Copy a command or its output with one click, collapse long output, and jump between blocks
  with `Ctrl+↑` / `Ctrl+↓`.

### Tabs and panes

- Tabs live in a sidebar, named after the current folder, with a dot that shows how the last
  command ended.
- Tabs are grouped by git repo, with a header per project and loose terminals at the end. The `+`
  on a project header opens another agent there (in a new worktree or in the repo folder) or a
  plain terminal.
- Projects opened with **New project** (`Ctrl+Shift+O`) are remembered in `projects.json`, next to
  `config.toml`, and listed under "Guardados" until you remove them. One click reopens one. In
  that window, pick a recent repo or one next to the current one, or paste a path; tick several
  agents to launch them at once, each in its own worktree and branch.
- Worktrees are never deleted behind your back. Palette → "Limpiar worktrees de Orion" removes the
  ones no tab uses, skipping any with unsaved changes; a branch is deleted only once merged.
- Split any pane to the right or downward, each with its own shell.
- **Grafo tab**: the branch icon on a project header (or palette → "Ver grafo de ramas") shows the
  repo's branches and commits as a graph, with search (`Enter` jumps to the next match), ref chips
  and commit details. Fetch, pull, push, branches, stashes, merges and pull requests are there too,
  and the changes panel stages by file or by hunk before committing.
- A command palette (`Ctrl+Shift+P`) finds any action by name and shows its shortcut.
- Agent tabs show a small logo of their agent (a generic terminal for other commands), also in the
  agent menus and the palette. It takes the colors of your theme. The logos are simple shapes drawn
  for Orion, not official artwork ([`assets/agents`](assets/agents/README.md)).

### Built for coding agents

- **Agent profiles** open a tab with an agent already running, optionally in its own
  `git worktree` and branch, so several agents can work in parallel without stepping on each other.
  Worktrees live in `.orion/worktrees/` inside the repo (git-ignored on its own). Files git ignores,
  like `.env`, are copied over if you list them in a `.worktreeinclude` file at the repo root: one
  relative path per line, files or folders.
- **Orchestrator** (palette → "Orquestar tarea con agentes…"): describe a task, and it gets split
  into independent subtasks, each with a model suited to it. Review the plan, and Orion opens one
  agent per subtask, each in its own worktree and branch. Requires
  [Claude Code](https://code.claude.com), installed and signed in.
- **Alerts**: a tab shows a pulsing dot when an agent finishes or asks for permission, even if
  you're looking at another tab.
- **`Shift+Enter`** adds a new line, for multi-line prompts.
- **Drag files or images** onto the window to type their paths.
- **`Ctrl+click`** opens links in your browser, and paths such as `src/app.rs:42` in your editor,
  at that line.
- **Clipboard**: what a program copies inside the terminal reaches your system clipboard.
  Programs can't read your clipboard.

### Other agents and executors

The orchestrator plans with Claude Opus and gives each subtask to the cheapest executor that fits
it. Besides the three Claude models, you can add any other CLI (OpenCode with DeepSeek, Junie…).
Before launching, each subtask has a menu to switch its executor. See
[Configuring agents](#configuring-agents) for how to set them up.

### Everything else

- **Suggestions from your history** as you type; `→` accepts them.
- **Themes**: Orion (night sky) by default, a monochrome option with color only where it means
  something, plus five more dark and light themes.
- **Translucent background**: Mica or acrylic on Windows 11, native vibrancy on macOS.
- **Any font**: pick an installed one, or drop a `.ttf` / `.otf` file onto the window.
- **Several shells**: PowerShell, Windows PowerShell, cmd and WSL on Windows; zsh, bash, fish
  and PowerShell on macOS.
- **A complete terminal**: 24-bit color, mouse support, full-screen apps, selection and clipboard.
- **Updates itself**: Orion lets you know when a new version is out and installs it in one click.

## Install

### Windows

1. Download `orion-<version>-windows-x64.zip` from
   [Releases](https://github.com/1Bnja/orion/releases/latest).
2. Unzip it anywhere and open `orion.exe`.

Nothing gets installed and nothing else is needed. To remove it, delete the folder (and, if you
want, your settings in `%APPDATA%\orion`).

### macOS

1. Download `orion-<version>-macos.zip` from
   [Releases](https://github.com/1Bnja/orion/releases/latest). It's a single app for Apple
   Silicon and Intel, on macOS 11 or later.
2. Unzip it and drag `Orion.app` into **Applications**, so it can update itself.
3. The first time, open it with **right-click → Open** and confirm.

That extra step is because Orion isn't signed with an Apple certificate, so macOS can't verify
the developer. You only do it once; updates don't ask again. On macOS 15 or later, try to open it
once, then go to **System Settings → Privacy & Security** and click **Open Anyway**. You can also
run this in Terminal:

```sh
xattr -dr com.apple.quarantine /Applications/Orion.app
```

To remove it, move `Orion.app` to the Trash (and, if you want, your settings in
`~/Library/Application Support/orion`).

## Shortcuts

On macOS, use `Cmd` where the table says `Ctrl`, except `Ctrl+C` to interrupt and `Ctrl+Tab` to
switch tabs. `Cmd+1` … `Cmd+9` go to a tab.

| Shortcut | What it does |
|---|---|
| `Ctrl+Shift+P` | Command palette |
| `Ctrl+,` | Settings |
| `Ctrl+Shift+T` | New terminal (the ▾ or a right-click on "+ Nueva pestaña" opens the menu: other shell, project, agent) |
| `Ctrl+Shift+O` | New project: a repo and an agent, optionally in its own worktree |
| `Ctrl+Tab` / `Ctrl+Shift+Tab` | Next / previous tab, in the order the sidebar shows them |
| `Alt+1` … `Alt+9` | Go to that tab (by its position in the sidebar) |
| `Ctrl+Shift+D` / `Ctrl+Shift+E` | Split the pane to the right / downward |
| `Ctrl+Alt+←↑→↓` | Move to the neighboring pane |
| `Ctrl+Shift+W` | Close the pane (or the tab, if it's the last pane) |
| `Ctrl+↑` / `Ctrl+↓` | Jump to the previous / next block |
| `Ctrl+C` | Copy the selection or the marked block; with nothing selected, interrupt |
| `Ctrl+Shift+C` / `Ctrl+Shift+V` | Copy / paste |
| `→` | Accept the suggestion (`F2` shows several in a list) |
| `Shift+Enter` | New line without running |
| `Ctrl+click` | Open the link or path under the mouse |

## Settings

Change everything from `Ctrl+,`, or edit `config.toml`:

- Windows: `%APPDATA%\orion\config.toml`
- macOS: `~/Library/Application Support/orion/config.toml`

Changes apply as soon as you save the file, without restarting.

```toml
theme = "Orion"         # Orion, Vercel, Tokyo Night, Catppuccin Mocha, Dracula, Gruvbox, Catppuccin Latte
shell = "PowerShell"    # the shell for new tabs
backdrop = "Mica"       # Ninguno, Mica, Acrílico
check_updates = true    # look for a new version on launch

[font]
family = "Cascadia Mono"  # an installed font, or the path to a .ttf/.otf file
size = 14.0
line_height = 1.2         # 1.0 = tight lines, 1.5 = very spaced out

# An agent profile: shows up in the palette and in the "+ Nueva pestaña" menu.
# Claude Code, Gemini CLI, Codex, OpenCode and Aider need no profile: Orion finds them on its own.
[[agent]]
name = "My agent"
command = "my-agent"    # what gets typed into the shell to launch it
worktree = true         # first create a git worktree in .orion/worktrees/ on a new branch (orion/my-agent-1)
```

Blocks and folder titles work in PowerShell and zsh, and history suggestions in PowerShell.
In other shells, Orion works as a regular terminal. Paths opened with `Ctrl+click` go to your
editor (`code`, if installed) or to the default app.

> Saving from the Settings window rewrites `config.toml` from scratch, so comments you added by
> hand are not kept. If you edit the file yourself, keep your notes somewhere else.

## Configuring agents

Agents are set up in `config.toml` (see [Settings](#settings) for where it lives). There are two
kinds of entries, and you can use either or both. Changes apply when you save the file.

### Agent profiles: `[[agent]]`

A profile adds an entry to the command palette and to the "+ Nueva pestaña" menu. Orion opens a tab
and types `command` into your shell.

```toml
[[agent]]
name = "Claude Sonnet"
command = "claude --model claude-sonnet-5-5"
worktree = true
```

| Key | What it does |
|---|---|
| `name` | How it appears in the menu and on the tab. Required. |
| `command` | What is typed into the shell to launch it. Anything you can run in a terminal works. Required. |
| `worktree` | Optional, `false` by default. With `true`, Orion first creates a git worktree in `.orion/worktrees/` on a new branch (`orion/<name>-1`, `-2`…), so several agents can work in parallel. |

Claude Code, Gemini CLI, Codex, OpenCode and Aider need no profile: Orion finds them on its own. Add
one only to pass options (a model, a flag) or to turn on `worktree`. A profile with the same
`command` as a detected agent replaces it in the menu.

To copy files git ignores into each new worktree (a `.env`, for example), list them in a
`.worktreeinclude` file at the repo root: one relative path per line, files or folders. Absolute
paths and `..` are skipped.

### Orchestrator executors: `[[executor]]`

The orchestrator (palette → "Orquestar tarea con agentes…") splits a task into subtasks and gives
each one to the cheapest executor that fits it. The three Claude models (`claude-opus-5-5`,
`claude-sonnet-5-5` and `claude-haiku-5-5`) are always available. Add other agents like this:

```toml
# OpenCode with DeepSeek (list the models you have with `opencode models`)
[[executor]]
id = "deepseek-pro"
use_for = "regular coding: a function, a fix, tests; cheap"
command = "opencode -m deepseek/deepseek-v4-pro --prompt {prompt}"

# Junie (JetBrains)
[[executor]]
id = "junie"
use_for = "changes that touch several files and need to understand the project"
command = "junie --model=mix.normal --prompt={prompt}"
```

| Key | What it does |
|---|---|
| `id` | The name the plan and the window show. With the same `id` as a Claude model, it replaces that one. |
| `use_for` | A sentence telling the planner when to pick it. The clearer it is, the better the choice. |
| `command` | What is typed into the shell. `{prompt}` is replaced by the subtask's prompt. |

- `{prompt}` becomes a single line in quotes: line breaks are flattened, double quotes turn into
  single quotes, and `!` and `%` are removed. If `command` has no `{prompt}`, the prompt is added at
  the end.
- Only executors whose program (the first word of `command`) is installed are offered to the
  planner. If one is missing, the plan window says which and why.
- The command must open the agent in **interactive** mode with the prompt already sent. Programs
  that run a task and exit won't work, because the next round starts when you close the agent
  (`/exit`).
- The agent must finish by **committing** on its branch. Without a commit, the subtasks that depend
  on it stay blocked.
- Each subtask has a menu in the plan window to switch its executor before launching.

To check a command before relying on it, replace `{prompt}` with a short text and run it in a
normal Orion tab: it should open the agent with that text already submitted.

### Let an AI agent set it up

You can ask any coding agent to edit the file for you. Paste something like this, with your
agent's name and command:

```text
Edit my Orion config.toml (Windows: %APPDATA%\orion\config.toml; macOS:
~/Library/Application Support/orion/config.toml). Keep every existing entry and add:
- an [[agent]] with name = "<name>" and command = "<command>"
- an [[executor]] with id = "<id>", use_for = "<when to use it>" and command = "<command with {prompt}>"
The command must open the agent in interactive mode with the initial prompt.
Then check that the file is still valid TOML.
```

## License

[MIT](LICENSE)
