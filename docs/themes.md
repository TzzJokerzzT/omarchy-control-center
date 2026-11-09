# Themes

A theme is a small set of **semantic slots**, not a list of every widget. `accent` is
whatever the interface uses to draw attention; `error` is whatever it uses to say something
failed. That indirection is why a theme can be twelve lines long and still colour the whole
program consistently.

## Built-in themes

| Id | Name | Character |
| --- | --- | --- |
| `default` | Terminal default | Uses the terminal's own palette and leaves its background alone |
| `zelda` | The Legend of Zelda | Hyrule at dusk: parchment, forest green, Triforce gold |
| `cyberpunk` | Cyberpunk | Night City signage: hot yellow on deep purple, cyan and magenta |
| `rei` | Rei Ayanami | Evangelion Unit-00: pale blue, hospital white, a red that is not warm |
| `monokai` | Monokai | The editor palette everyone recognises |
| `matrix` | Matrix | Green phosphor on black, and nothing else |
| `cviper` | C. Viper | Street Fighter 6: charcoal suit, white shirt, an orange that hits |

Open the selector with `t`, move with `↑`/`↓` (the list is the preview), press `Enter` to
apply and remember it, `Esc` to keep the previous one.

`default` paints no background on purpose: with it, the program looks like the rest of your
terminal. Every other theme paints its own.

## Choosing a theme without the selector

The choice made in the selector is stored in
`~/.local/state/omarchy-control-center/preferences.toml`:

```toml
theme = "matrix"
```

To pin a theme regardless of what was picked later, set it in
`~/.config/omarchy-control-center/config.toml` instead:

```toml
theme = "cyberpunk"
```

Priority: **`config.toml` wins**, then the remembered choice, then `default`. The interface
never rewrites your `config.toml` — that file carries your comments, and a keystroke should
not destroy them.

## Writing your own

Drop a `.toml` file into `~/.config/omarchy-control-center/themes/`. The **file name is the
theme id**, so `sunset.toml` becomes `theme = "sunset"`. It appears in the selector
immediately.

```toml
# ~/.config/omarchy-control-center/themes/sunset.toml
name = "Sunset"
author = "your name"                  # optional
description = "Warm, low contrast"    # optional, shown in the selector

[colors]
# Optional: leaving these out keeps the terminal's own colours.
background = "#201010"
foreground = "#ffeee0"
selection  = "#3a1f1a"

# Required.
muted   = "#806058"    # labels, hints, paths
accent  = "#ff8a5c"    # titles, the focused pane, the caret
success = "#8fd18f"
warning = "#ffc857"
error   = "#ff5c5c"
```

### The rules

* Colours are `#rrggbb` (`#rgb` also works). Anything else is rejected with the field named.
* `muted`, `accent`, `success`, `warning` and `error` are required. `background`,
  `foreground` and `selection` are optional; without them the terminal's colours are used.
* Unknown keys are **rejected**, not ignored: `accentt = "..."` is a typo, and silently
  ignoring it would leave you wondering why nothing changed.
* You cannot shadow a built-in id. `matrix.toml` is refused; rename it.
* A file that fails to load is reported in the selector with the exact reason, and the rest
  of the program keeps working.

A good way to start is to copy a built-in from
[`src/ui/theme.rs`](../src/ui/theme.rs) and change the hex values.

## What a theme cannot do

A theme changes colours only. It cannot change layout, hide a confirmation, or alter what an
operation does — those are not presentation. If a theme could do that, choosing a theme would
be a way to make a destructive action less explicit.

## Screen colours and accessibility

The interface never relies on colour alone: every state is also a word (`orphan`, `missing`,
`active`, `disabled`), and the selected row is marked with a marker as well as reverse video.
A theme with low contrast is a choice, not a trap.
