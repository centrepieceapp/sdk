# Centrepiece extension SDK

Write extensions for [Centrepiece](https://github.com/centrepieceapp), the
keyboard-driven command bar for macOS. An extension is a WebAssembly component
that owns one prefix — the word typed to enter it — the rows under it, and what
happens when one is picked. Centrepiece runs each one in a sandbox that
reaches only what its manifest asks for.

This repository is the contract between Centrepiece and its extensions:

```
wit/extension.wit   the contract itself, as a WIT world
src/lib.rs          centrepiece-extension, the Rust crate over it
```

The official extensions, at
[centrepieceapp/extensions](https://github.com/centrepieceapp/extensions), are
built with it and are the best worked examples.

## Versions

The crate's version follows the extension API: `0.6.x` speaks API `0.6`, the
version of the WIT package and the `api` an extension's manifest names.
Centrepiece loads an extension only if it speaks the extension's API; while
the major version is 0, every minor version is a breaking change, so an
extension built for 0.5 has to be rebuilt for 0.6.

Releases are git tags, `v0.6.0` and so on. Depend on one:

```toml
[dependencies]
centrepiece-extension = { git = "https://github.com/centrepieceapp/sdk", tag = "v0.6.0" }
```

What changed in each API version:

* **0.6** — `tint` on a row: the colour its builtin or asset icon is drawn in,
  instead of the theme's.
* **0.5** — plugins became extensions: the manifest is `extension.toml`, the
  component `extension.wasm`, and this crate `centrepiece-extension`.

## An extension

An extension is a folder:

```
emoji/
  extension.toml  id, name, prefix, icon, and what it may reach
  extension.wasm  the component
  assets/         SVG icons, named from the extension by file name
```

The manifest:

```toml
id = "emoji"                 # lowercase letters, digits and dashes
name = "Emoji"
description = "Copy an emoji"
prefix = "em"                # one lowercase word
icon = "builtin:clipboard"   # or "asset:smile.svg", from assets/
api = "0.6"
# wasm = "extension.wasm"    # the default

[permissions]
clipboard = true
```

The id names the extension's keychain entries, its data folder and its
section of `centrepiece.yml`. Neither the id nor the prefix can be one a
built-in extension already has (`apps`, `clipboard` and `install`; `cb` and
`ext`), and the prefix cannot be one another installed extension answers to.

Centrepiece loads every extension in `~/.config/centrepiece/extensions/` (under
`$XDG_CONFIG_HOME` when that is set) at startup, plus any folder passed as
`--extension <folder>`. **Install extension ▸ From URL** in Centrepiece
installs one from an `https://` URL of a `.tar.gz` or `.zip` holding the
folder, at the top or inside a single folder, and starts it straight away.
Users can switch one off, hand it settings, or give it a hotkey in
`centrepiece.yml`:

```yaml
extensions:
  emoji:
    enabled: false
  github:
    settings: {}         # given to the extension as JSON
    hotkey: alt-cmd-g    # opens Centrepiece with the extension already entered
```

## What an extension can and cannot do

Each extension runs in its own [Wasmtime](https://wasmtime.dev) instance, on a
thread of its own. It sees no files but its data folder
(`~/.local/state/centrepiece/extensions/<id>/`, mounted at `/data`), no network, no
environment, and nothing else of Centrepiece's. Everything else goes through
Centrepiece, and only as far as `[permissions]` in its manifest allows:

```toml
[permissions]
network = ["api.github.com", "*.example.com"]   # HTTP to these hosts; redirects are not followed
secrets = true         # its own keychain entries, apart from every other extension's
clipboard = true       # writing to the clipboard
read-clipboard = true  # the text on the clipboard when Centrepiece was summoned
front-app = true       # the application in front when Centrepiece was summoned
open = true            # opening URLs and files
system-actions = true  # locking, sleeping, starting the screen saver
read = ["~/Library/Application Support/Google"]   # these folders, read-only
run = ["/Applications/Tool.app/Contents/MacOS/Tool"]   # starting or running these programs
```

Folders under `read` are mounted read-only at the same paths as on the Mac,
and an extension granted any also sees `HOME`, to find them; one that does not
exist when the extension starts stays out of reach until it restarts. Programs
under `run` are only ever started by the exact path listed: `host::run` starts
one detached, with no input or output, and `host::exec` runs one to the end
and hands back its exit code and what it printed. Both take absolute paths, or
`~/` for the home folder, and never `..`.

A call that runs past two seconds is stopped; an extension that traps or is
stopped is started afresh on the next call, and after three failures it stays
off until Centrepiece restarts. Centrepiece waits at most 30 ms for any answer,
so a slow extension shows its results late rather than holding up typing.

Users see what an extension may use before they install it, so ask for no
more than it needs.

## Writing one

Extensions are Rust crates against this one, built for `wasm32-wasip2`
(`rustup target add wasm32-wasip2`), with `crate-type = ["cdylib"]`:

```rust
use centrepiece_extension::{Extension, Item, Response, Screen, export_extension, host};

struct Emoji;

impl Extension for Emoji {
    fn new() -> Self {
        Emoji
    }

    fn activate(&mut self) -> Response {
        let items = vec![Item::new("🎉", "party").glyph('🎉')];
        // A fixed list: Centrepiece filters and ranks it, and remembers picks.
        Response::Replace(Screen::search(items).host_filtered())
    }

    fn select(&mut self, id: &str) -> Response {
        host::copy(id);
        Response::Dismiss
    }
}

export_extension!(Emoji);
```

`cargo build --release --target wasm32-wasip2` makes the component; copy it to
`extension.wasm` beside the manifest. `cargo doc --open` documents the whole
API. The shapes worth knowing:

* **Screens.** `Screen::search` filters as you type; `host_filtered()` has
  Centrepiece do the filtering and ranking. `Screen::menu` is a fixed list
  where each row's `key` picks it outright. `Screen::prompt` collects one
  value, optionally masked, and delivers it to `Extension::submit`.
* **Responses.** `Replace` redraws, `Push` opens a screen that `backspace`
  comes back from, `Pop` goes back, `Dismiss` closes Centrepiece, and `Error`
  reports a problem without disturbing the screen. An extension with a
  searchable screen on top of another learns of `backspace` through
  `Extension::popped`.
* **Slow work is a task.** `http::get(url).send()`, `host::read_secret` and
  friends return a `TaskId` at once; the result arrives in
  `Extension::task_finished`. `Tasks<T>` remembers what each id was for.
  The official [`github`](https://github.com/centrepieceapp/extensions/blob/main/github/src/lib.rs)
  extension chains pages and requests this way.
* **Shortcuts and offers are pushed.** Call `host::set_shortcuts` (in
  `started`) and `host::set_offers` (in `summoned`) instead of answering on
  every keystroke; Centrepiece keeps them, and waits a moment on every summon
  so the offers are there for the first frame. `host::clipboard_text` is what
  to offer for: [`chrome`](https://github.com/centrepieceapp/extensions/blob/main/chrome/src/lib.rs)
  offers the copied URL this way.
* **Suggestions answer the root.** `Extension::suggest` is asked what was typed
  at the root, before any prefix, and the rows it returns go first; picking one
  calls `select`, which may push a screen. It runs on every keystroke, so it
  has to be cheap: [`color`](https://github.com/centrepieceapp/extensions/blob/main/color/src/lib.rs)
  answers a color and nothing else.
* **Files are plain `std::fs`**, in the folders the manifest lets it read.
  `home_dir()` finds `~`.
* **Ranking is shared.** `host::rank` is Centrepiece's fuzzy matcher, weighted
  by `host::record_pick`, so an extension ranks the way the rest of Centrepiece
  does.
* **Icons** are `Icon::asset("repo.svg")` from the extension's `assets/` —
  single-colour SVGs, tinted to match the theme — or `Icon::builtin(name)`,
  one of Centrepiece's own: `bookmark`, `clipboard`, `link`, `refresh`,
  `document`, `file`, `settings`, `chevron-right`, `computer`, `shield-lock`,
  `bed`, `screen-saver`, `layout-grid`, `move-window`, `switch-layout`,
  `tiles-horizontal`, `tiles-vertical`, `accordion-horizontal`,
  `accordion-vertical`, `package`, `download`, `globe` and `browser`.

The contract is [`wit/extension.wit`](wit/extension.wit), so an extension can
be written in anything that compiles to a WebAssembly component; this crate is
a convenience over it.

To work on one, point Centrepiece at the folder and rebuild as you go; it
picks up a rebuilt component the next time you enter the extension:

```sh
/path/to/Centrepiece.app/Contents/MacOS/centrepiece --replace --show --extension path/to/emoji
```

Keep the extension's own logic in plain Rust, and its tests run natively with
`cargo test`.

## Changing the contract

Centrepiece builds its side of the contract from `wit/extension.wit` here, as
a submodule, so a change lands in three steps: change and tag this repository,
move Centrepiece's submodule to the tag, then move the official extensions'
dependency to it. A change that breaks extensions bumps the minor version
(the major, from 1.0) in the WIT package, `Cargo.toml` and the list above.
