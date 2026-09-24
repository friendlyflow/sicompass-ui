# Project Instructions

sicompass-ui was split out of the
[sicompass](https://github.com/friendlyflow/sicompass) workspace. Its git
history before that point is the history of `src/sicompass-ui` there, and before
that of the same files in the app crate (`src/sicompass/src`, and earlier
`src/sicompass-rs/src`). Work on it is usually driven from a sicompass checkout
next to this one (`../sicompass`), whose `/commit-and-push`, `/release`, `/sync`
and `/update-cargo` take this repo's name as their first argument and then
follow the skills in this repo's `.claude/skills/`.

## Environment (Nix: NixOS, other Linux, and macOS)

The toolchain comes from the flake dev shell in [flake.nix](flake.nix), which
is sicompass's dev shell minus the application-only tools. Nothing is installed
system-wide.

- **Check once per session**, then stick with the answer: `command -v cargo`.
  - Non-empty: the shell is inside `nix develop`, so run `cargo ...` directly.
  - Empty: prefix every toolchain command with `nix develop -c`.
- `nix develop -c <cmd>` prints a `warning: Git tree ... is dirty` line on
  stderr first. That warning is noise, not a failure.
- Evaluate the flake through `git+file://$PWD`, never a plain path (a plain path
  copies `target/` into the store and hangs), and always under `timeout`.
- The dev shell is platform-split exactly like sicompass's, and the same rules
  hold: never reference a Linux-only package (`wayland`, `mesa`,
  `at-spi2-core`) outside the `isLinux` branch, since nixpkgs marks `wayland`
  bad on darwin and a stray reference breaks `nix develop` at eval time there.
  `x86_64-darwin` builds from the `nixpkgs-26.05-darwin` input.
- `.cargo/config.toml` points every cargo-spawned process at a throwaway XDG
  tree under `target/`. Keep it. This crate links `sicompass-sdk`, whose
  `platform` module would otherwise have tests reading and writing the real
  sicompass directories.
- The version lives in `[package] version` in `Cargo.toml`, and `flake.nix`
  reads it from there.

## How this crate is consumed

It is **not on crates.io**. sicompass and loginsicompass depend on it by git:

```toml
sicompass-ui = { git = "https://github.com/friendlyflow/sicompass-ui", tag = "vX.Y.Z" }
```

A `rev = ...` pin stands in for the tag between releases. To work on this crate
and an embedder together, uncomment the embedder's
`[patch."https://github.com/friendlyflow/sicompass-ui"]` section, which points
at `../sicompass-ui`. That section must be commented out again before the
embedder commits. From sicompass, `/sync all` builds both embedders against the
local checkout, so a change that breaks either one is caught before it is
tagged.

**Keep dependency versions in step with sicompass's `[workspace.dependencies]`.**
The app links this crate, so a requirement that diverges (a different `ash`,
`sdl3` or `accesskit` major) makes cargo build two copies into the app, and
types stop lining up at the boundary.

## Generated files that are committed

- `shaders/*.spv` and `shaders/CHECKSUMS`:
  [scripts/gen-shaders.sh](scripts/gen-shaders.sh). Rerun after editing any
  `shaders/*.vert` or `*.frag`, and commit the output with the source.
  `cargo test` fails if you forget. The script's header explains why this is
  not a `build.rs` step.
- `assets/icon-256x256.png` is a **copy** of sicompass's
  `assets/icons/256x256.png`, which sicompass's `scripts/gen-icons.sh`
  generates. When the icon changes there, copy it here and tag a release.
  sicompass's `tests/packaging.rs` fails while the two differ.
- `THIRD-PARTY-LICENSES.html`: `cargo about generate about.hbs`. The `licenses.yml`
  workflow fails if it drifts.

## Architecture: the boundary (hard rule)

This crate is the renderer shared by two binaries: the `sicompass` application
and the `loginsicompass` greetd greeter. It holds the SDL3 window, the Vulkan
device, font rasterisation, the list layout, the key handlers, the insert-mode
text field and the AccessKit bridge. What only an *application* has (the
provider catalogue, `settings.json`, the WASM plugin host, the self-updater,
the Windows Start Menu entry) stays in sicompass.

**This crate must not depend on `sicompass-builtins`, `sicompass-updater`,
`wasmtime` or `reqwest`, nor import any provider crate.** That is the rule the
split exists to enforce: linking the application into a login screen cost 465
crates, including a bundled SQLite, a headless-Chromium driver, an IMAP client
and an SMTP client, none of which a login screen ever calls. The Stop hook
`.claude/hooks/check-no-heavy-deps.sh` checks this at the end of every turn.

Where the renderer needs something only the embedder can answer, it asks:

- `registry::HostHooks`: six methods, every one defaulting to a no-op, stored
  on `AppRenderer`. The app installs its own; the greeter takes the defaults,
  which are all correct for something with no settings file, no updater and no
  tabs.
- `http::register_body_fetcher`: an HTTP client for following `<link>` and for
  `<image>` values that are URLs. Same shape as
  `sicompass_sdk::register_url_fetcher`. Unregistered, an HTTP link reports that
  it cannot be followed and the node still renders.
- `app_state::AppConfig`: everything the window used to hardcode (title,
  `app_id`, size, custom titlebar, maximized, fullscreen, icon, font scale).
  `Default` reproduces the application exactly, and a test asserts it, so a
  field added here must default to whatever the line it replaced did.

Because `AppState` belongs to this crate, Rust's orphan rule stops an embedder
adding an inherent `AppState::new()`. sicompass's startup is the free function
`boot::app_state()` for that reason.

## Architecture: standalone binaries

Shaders (`src/shaders.rs`), fonts (`src/fonts.rs`) and the window icon
(`src/icon.rs`) are compiled in with `include_bytes!`, so nothing is ever read
relative to the executable. Every embedder has to ship the font license texts,
since the fonts are inside its binary. `fonts::LICENSES` exposes them so an
embedder can check its shipped copies.

## Code Style

Follow standard Rust idioms. Use `#[allow(...)]` sparingly and only when
justified. In `README.md`, do not use em dashes or semicolons. Use commas
instead, or split into separate sentences.

## Testing

- `cargo test`. Tests that open an SDL window need a display: use
  `xvfb-run -a cargo test` on a headless machine, as CI does.
- `cargo clippy --all-targets`. Lints carried over from the workspace are not
  denied yet, so do not add new ones.
- After implementing changes, always run the tests before finishing. When
  adding new code, write or update tests. If tests fail, fix the code.

## Test Integrity

- Never remove or weaken test assertions to make a failing test pass. Fix the
  code instead.
- If a test itself is genuinely wrong and needs changing, **ask the user
  first** before modifying it.

## Releasing

A release is a `vX.Y.Z` tag on `main`. See `.claude/skills/release/SKILL.md`.
After tagging, move the `sicompass-ui` pin in sicompass and loginsicompass to
the new tag.
