# PRmator

Fast GitHub PR review in a native GPUI app, with Claude one right-click away.

```
┌ PRs / files ┐┌ diff (virtualized) ─────────────┐┌ Claude ──────────┐
│ To review   ││ ▾ M src/batch.rs   +12 −40  ☐   ││ batch.rs · L12–20│
│ Mine        ││ @@ -10,6 +10,9 @@               ││ Find bugs…       │
│ Involved    ││ 10 10   fn apply() {            ││ ▸ Bash gh pr diff│
│             ││ 11    - old()      ← right-click││ The loop at …    │
└─────────────┘└─────────────────────────────────┘└──────────────────┘
        prmator (GPUI + tree-sitter)  ──ws://127.0.0.1:7788──  prmator-claude  ──stdio──  claude -p
```

## Run

Needs `gh` (logged in) and `claude` on your PATH.

```sh
cargo build --release                    # builds both binaries
./target/release/prmator                 # PR list
./target/release/prmator owner/repo#123  # or a github.com/…/pull/123 URL
```

### As a macOS app

```sh
scripts/make-app.sh                     # builds PRmator.app → ~/Apps/PRmator.app
DEST=/Applications scripts/make-app.sh  # or anywhere else
```

The bundle contains both binaries, the icon (`assets/icon/`, drawn by `make_icon.swift`) and
an ad-hoc signature. When it's opened from Finder or the Dock, PRmator reads your login
shell's `PATH`, so `gh` and `claude` are found just like in a terminal.

The app starts `prmator-claude` (the background Claude bridge) next to itself and stops it on quit.
To keep one running yourself: `./target/release/prmator-claude`. The app connects to an
existing bridge if one is already listening.

## Reviewing

- **Conversation first:** opening a PR shows its description, labels, reviewers, checks and the comment timeline (comments, reviews, inline review threads; click a thread to open it in the diff). **Start review** (or the *Files changed* tab, or `⌘E`) enters review mode, and `⌘E` / *Exit review* goes back.
- **📖 Story (virtual commits):** Claude retells the PR as an ordered series of small, atomic commits, each with the *why* and what comes next, ordered so each one builds on the last (foundations → behavior → wiring → tests). No code is added or removed: commits only regroup the PR's existing hunks, and PRmator checks they cover every hunk exactly once. The Story tab shows one commit at a time; the other commits' changes in the same files show as `↪ part of commit N` markers. The `↑` / `↓` expanders and **⇕ Full file** work there too, so you can read the code around a commit without leaving the story. `.` / `,` move between commits, the left column lists them like a git log, and progress is saved.
- **Review comments:** hover a line and click the **+** in the gutter, right-click → *Add comment*, or select lines and press `m`. Write in the comment box (markdown; ↵ new line, ⌘↵ add to review, ⇧⌘↵ post now). Drag its header to move it and its right or bottom edge or corner to resize it, or use the keys below. It reopens where you left it; double-click the header or press `⌘0` to put it back next to the line. **Add to review** saves a draft (kept per PR until posted); **Comment now** posts right away. Existing GitHub threads and your drafts show as 💬 rows under their line; click one to read it or reply. **Submit review · N** posts every draft as one GitHub review with a summary and *Comment / Approve / Request changes*. Set `PRMATOR_DRY_RUN=1` to print the GitHub requests instead of sending them.
- **Settings (`⌘,` or ⚙):** fonts, sizes, line height and themes. The code font and theme default to your **Zed settings** (`~/.config/zed/settings.json`: `buffer_font_family/size`, `buffer_line_height`, `theme`); the interface uses the system font. Changing anything makes the settings PRmator's own; *Reset to Zed settings* follows Zed again. `prmator --settings` prints what's in effect.
- **Left:** PRs where your review is requested or that you've already reviewed (they stay listed after you approve), PRs you opened, PRs you're involved in, each with your saved progress (`step 3/9`, `5 viewed`). Each PR you've reviewed shows **✓ approved / ✓ changes requested / ✓ reviewed**, or **↻ re-review** (orange) when commits were pushed after your last review or the author asked you again; the lists refresh after you submit a review. **Stacked PRs** (one PR's base branch is another's head) are grouped into one card in merge order, numbered from 1 (merges first). Every member of a stack is shown (the ones in the list have a filled step number, the others are dimmed); click a stack's header to fold it to one line, which is remembered. A branch that more than 3 open PRs target counts as an integration branch, not a stack. `prmator --stacks [mine|involved]` prints the grouping. Opening a PR shows two tabs:
  - **Files:** each file's changed symbols (fns, structs, impls, tests…), worked out with tree-sitter. Click one to jump to it.
  - **Plan:** Claude's review route, shown as a pipeline. It starts at the entry point, follows the flow and ends with tests, and each step says what to check. Progress is saved, so `c` continues where you left off.
- **Diff:** **Unified** or **Split** (side by side) from the toolbar or `s`, with tree-sitter syntax colours computed from the full file at the merge base and the head, plus word-level change highlights.
- **Expand context:** the bars between hunks reveal hidden unchanged lines: `↑` above the next hunk, `↓` below the previous one (20 at a time), or *Expand all*. **⇕ Full file** on a file header shows the whole file. Revealed lines get syntax colours, hover cards and right-click like the rest.
- **📄 File view:** *View file* on a file header (or `f`, or *Open in file view* in the right-click menu) shows the whole file at the PR's head, without diff, next to the **project tree** (every file of the repo, not only the changed ones). Folders holding changed files have a dot and changed files their status letter; in a changed file, added and modified lines have a green/orange bar in the gutter and removed lines a red mark. Hover cards, the right-click menu and *Ask Claude* work there too, and *Open file* on a hover card opens definitions from anywhere in the repo. `f`, `esc` or *← Back to review* return to the diff.
- **Token selection:** in the diff and the file view, a click selects the whole token under the mouse (there is no caret inside a word), dragging extends token by token, and a double-click takes the line. `←` `→` move to the previous/next token (across lines), `↑` `↓` to the token in the same column above/below, and `⇧` extends; selections can span lines. Clicking the line numbers still selects whole lines. `⌘C` copies the selected text.
- **Hover** a type name to see its definition in a card. It looks in the PR's own files, then `$PRMATOR_CODE_DIR`, then GitHub code search, and the card has *Ask Claude*, *Jump to it* and *GitHub ↗*.
- **Right-click** a token, selection, hunk header or file header. The menu is split by purpose: **Navigate** (show the definition of the identifier, find it in the diff, open in file view), **Review** (add a comment), **Ask Claude** (a free question, *Explain*, *Find bugs & risks*, … and *What is `X`?*) and **Copy** (the selection, or the lines as a diff).
- **Ask Claude** opens a **floating chat** next to the code. You can go back and forth there (↵ sends, ⇧↵ adds a line, long questions wrap); each chat is its own Claude session. **Done** (`esc` / `⌘↵`) files it into the **review log** on the right. The log is a list of collapsible entries (code location, first question, turns), each with *Continue*, *Jump to code*, *Copy* and *Delete*, and it is saved per PR so you can come back to it.
- **Toolbar:** it doubles as the window titlebar (drag or double-click its empty areas), so content fills the window. It holds search, the Plan / Continue button, Unified/Split and the theme picker.
- **Themes:** all of Zed's themes (One, Ayu, Gruvbox) plus any Zed themes installed on your Mac (`~/.config/zed/themes` and theme extensions), previewed live as you browse. Pick one from the toolbar or with *Theme: select…*. The picked theme becomes your light or dark theme, and *Theme mode* (follow system / light / dark) chooses between them, as in Zed. `prmator --themes` lists what was found.

| Key | Action |
| --- | --- |
| `⇧⌘O` | search symbols in the PR (`@` prefix) |
| `⌘P` | go to file (or PR when none is open) |
| `⇧⌘P` / `⌘K` | commands (`>` prefix): theme, diff mode, plan, … |
| `⌘F` | search the diff text (`/` prefix) |
| `c` | continue review: plan the review, or go to the next unfinished step |
| `]` / `[` | mark step done and go to the next / previous step |
| `j` / `k` | next / previous file |
| `v` | mark file viewed (collapses it, jumps to the next unviewed file) |
| `s` | unified ⇄ side by side |
| `a` | ask Claude about the selection (opens the floating chat) |
| `⌘C` | copy the selected text (or the selected diff lines) |
| `esc` | close the card or menu, or clear the selection |
| `⌘E` | enter / exit review mode (Conversation ⇄ Files changed) |
| `.` / `,` | story: mark commit reviewed and go to the next / previous commit |
| `m` | comment on the selected lines |
| `f` | open / close the file view (full file + project tree) |
| `←` `→` / `↑` `↓` | select the previous / next token, or the one above / below (`⇧` extends) |
| `⌥←` `⌥→` / `⌘←` `⌘→` / `⌘↑` `⌘↓` | in any text field: by word / to line start, end / to text start, end (add `⇧` to select; `⌥⌫` `⌘⌫` delete) |
| `⌥⌘` arrows / `⇧⌥⌘` arrows | comment box: move / resize |
| `⌘=` / `⌘−` / `⌘0` | comment box: bigger / smaller / back next to the line |
| `⌥↑` `⌥↓` / `PgUp` `PgDn` | comment box: scroll the code and thread |
| `⌘,` | settings |
| `⌘R` / `⌘[` | refresh / back to the PR list |

Settings and per-PR progress (viewed files, plan, finished steps) are saved in
`~/Library/Application Support/PRmator/state.json`.

Debug the analysis without the UI: `prmator --analyze owner/repo#123 [TypeName…]` prints the
changed symbols per file, the plan outline sent to Claude, and definition lookups.

## Claude bridge

`prmator-claude` is a WebSocket server (protocol in `crates/protocol`). Each question runs one
`claude -p --output-format stream-json` turn, and the text is streamed back as `Delta` messages.
Claude can only run `gh pr view` and `gh pr diff`. It cannot comment, approve or push.

| Env var | Effect |
| --- | --- |
| `PRMATOR_CODE_DIR` | Folder with your local clones. If `$PRMATOR_CODE_DIR/<repo>` exists, Claude runs there and can also Read/Grep/Glob the code. |
| `PRMATOR_CLAUDE_ARGS` | Extra CLI args, e.g. `--model sonnet` |
| `PRMATOR_CLAUDE_BIN` | Path to the `claude` binary |
| `PRMATOR_BRIDGE_URL` | Connect to this bridge URL instead of starting one |

*Plan review* sends the changed files and symbols and runs Claude with `--json-schema`, so
the route comes back as structured steps (`file`, `symbol`, `why`, `check`, `related_files`).

Smoke-test a bridge without the UI:
`cargo run -p prmator-claude --example ask -- owner/repo 123 "What does this PR do?"`
(or `--plan < outline.txt`, with the outline printed by `prmator --analyze`).
