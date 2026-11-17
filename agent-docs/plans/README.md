# frext plans

This index lists every plan in execution order. A plan's number **is**
its position in the sequence: the completed set is always a contiguous
prefix, every dependency points at a lower number, and a plan may merge
only when every lower-numbered plan is already `complete`. A plan's
number is frozen once work on it starts.

Status moves one way: `stub` -> `planned` -> `active` ->
`pending merge` -> `complete`. A `stub` records goal and durable
decisions only; it is decomposed into subtasks when it is activated. A
`planned` plan has a full subtask breakdown awaiting sign-off. The PR
merging is what makes a plan `complete`; flip the status in this table
and in the plan document in the same commit.

## The TUI programme

frext is offered today as an egui GUI. Plans 01-09 add a second,
terminal front end, shipped as its own binary, `frext-term`, with a UI
as close to 1:1 with the GUI as a terminal allows: tabs, the file-tree
sidebar, syntax highlighting, find/replace, and spelling/grammar
squiggles.

The programme rests on one structural decision: all editor logic moves
into a frontend-neutral library crate, `frext-core`, and both binaries
are thin front ends over it.

```text
frext-core   (lib)  tabs, persistence, workspace, search, proofreading,
                    highlighting spans, palette, the Editor controller
frext        (bin)  egui/eframe GUI           -> depends on frext-core
frext-term   (bin)  ratatui/crossterm TUI     -> depends on frext-core
```

`frext-core` must never depend on egui, eframe, ratatui or crossterm.
That boundary is what keeps the two front ends honest.

| No. | Plan                                                              | Status        | Depends on |
| --- | ----------------------------------------------------------------- | ------------- | ---------- |
| 01  | [Core extraction (`frext-core`)](01-core-extraction.md)           | complete      | none       |
| 02  | [`frext-term` shell](02-term-shell.md)                            | complete      | 01         |
| 03  | [TUI buffer view (editor widget)](03-term-buffer-view.md)         | complete      | 02         |
| 04  | [TUI find and replace](04-term-find-replace.md)                   | complete      | 03         |
| 05  | [TUI file-tree sidebar](05-term-file-tree.md)                     | complete      | 02         |
| 06  | [Terminal capabilities and undercurl](06-term-capabilities.md)    | complete      | 02         |
| 07  | [TUI proofreading](07-term-proofreading.md)                       | pending merge | 03, 06     |
| 08  | [TUI file icons (kitty images, Nerd Font)](08-term-file-icons.md) | stub          | 05, 06     |
| 09  | [`frext-term` packaging and docs](09-term-packaging.md)           | stub          | 02         |

## Concurrency

Work on adjacent plans may overlap where both the plan-graph test (no
dependency either way) and the code-level test (disjoint files,
disjoint behaviour, no shared-type edits) pass. Merges are still
strictly in number order.

- 05 (file tree) and 06 (capabilities) touch modules disjoint from 03/04
  (the buffer view) and may be worked concurrently with them once 02 has
  merged. Each must re-check for shared-type edits in `frext-core` at
  activation.
- 09 (packaging) only needs 02 and can be prepared early, but merges
  last.
