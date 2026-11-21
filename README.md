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
- Split any pane to the right or downward, each with its own shell.
- A command palette (`Ctrl+Shift+P`) finds any action by name and shows its shortcut.

### Built for coding agents

- **Agent profiles** open a tab with an agent already running, optionally in its own
  `git worktree` and branch, so several agents can work in parallel without stepping on each other.
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

### Everything else

- **Suggestions from your history** as you type; `→` accepts them.
- **Themes**: a monochrome default with color only where it means something, plus five more,
  dark and light.
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
| `Ctrl+Shift+T` | New tab (right-click "+ Nueva pestaña" to pick the shell) |
| `Ctrl+Tab` / `Ctrl+Shift+Tab` | Next / previous tab |
| `Alt+1` … `Alt+9` | Go to that tab |
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
theme = "Vercel"        # Vercel, Tokyo Night, Catppuccin Mocha, Dracula, Gruvbox, Catppuccin Latte
shell = "PowerShell"    # the shell for new tabs
backdrop = "Mica"       # Ninguno, Mica, Acrílico
check_updates = true    # look for a new version on launch

[font]
family = "Cascadia Mono"  # an installed font, or the path to a .ttf/.otf file
size = 14.0
line_height = 1.2         # 1.0 = tight lines, 1.5 = very spaced out

# An agent profile: shows up in the palette and when you right-click "+ Nueva pestaña".
[[agent]]
name = "My agent"
command = "my-agent"    # what gets typed into the shell to launch it
worktree = true         # first create a git worktree on a new branch (orion/my-agent-1)
```

Blocks and folder titles work in PowerShell and zsh, and history suggestions in PowerShell.
In other shells, Orion works as a regular terminal. Paths opened with `Ctrl+click` go to your
editor (`code`, if installed) or to the default app.

## License

[MIT](LICENSE)
