# sicompass-ui

*The list renderer behind Sicompass and its login screen.*

sicompass-ui is part of [Sicompass](https://github.com/friendlyflow/sicompass),
a keyboard-first, accessibility-first way to use your entire computer. Every
Sicompass screen is one list, a single layer of a hierarchy, and this crate is
what draws that list and speaks it.

It turns a tree of items into pixels and into an accessibility tree at the same
time:

- an SDL3 window with a Vulkan renderer (MoltenVK on macOS)
- font rasterisation through FreeType, with fonts, shaders and the window icon
  compiled in, so a binary that links it needs no files beside it
- the list layout, the key handlers and the shared text field
- a screen-reader bridge through [AccessKit](https://github.com/AccessKit/accesskit),
  which tags each item with its language so it is spoken in the right voice

Two programs use it: the [Sicompass application](https://github.com/friendlyflow/sicompass)
and [loginsicompass](https://github.com/friendlyflow/loginsicompass), the login
screen. The login screen is why this is its own crate. It needs the same lists
and the same screen-reader support as the app, and none of the app's network,
browser, mail or plugin code.

## Using it

It is not published on crates.io. Depend on it by git tag:

```toml
[dependencies]
sicompass-ui = { git = "https://github.com/friendlyflow/sicompass-ui", tag = "v0.2.0" }
```

Content comes from a [`sicompass_sdk::Provider`](https://crates.io/crates/sicompass-sdk).
Anything only the embedding program can answer (a settings file, tabs, an HTTP
client) is asked through `registry::HostHooks` and
`http::register_body_fetcher`, which default to doing nothing.

Every program that links this crate ships the fonts inside its binary, so it
has to ship their license texts too: `fonts/LICENSE-DejaVu.txt` and
`fonts/LICENSE-NotoColorEmoji.txt`. `fonts::LICENSES` holds both.

## Building from source

```bash
nix develop          # optional, brings the whole toolchain
cargo build
xvfb-run -a cargo test
```

Some tests open a window, so on a machine without a display they run under
`xvfb-run`. It runs on Linux, macOS and Windows.

The compiled shaders in `shaders/*.spv` are committed. After editing a
`shaders/*.vert` or `*.frag` file, run `scripts/gen-shaders.sh` inside
`nix develop` and commit the result. `cargo test` fails if you forget.

## Related repositories

- [sicompass](https://github.com/friendlyflow/sicompass), the application
- [loginsicompass](https://github.com/friendlyflow/loginsicompass), the login
  screen
- [sicompass-plugin-sdk](https://github.com/friendlyflow/sicompass-plugin-sdk),
  the SDK that defines `Provider`

## Community

Join the conversation on
[Discord](https://discord.com/channels/1464152138753249313/1464152139231137894).

## License

#### Open source license

If you are creating an open source application under a license compatible with
the GNU GPL license v3, you may use this project under the terms of the GPLv3.
See [LICENSE](LICENSE).

The bundled fonts keep their own licenses: DejaVu under the Bitstream Vera /
DejaVu license and Noto Color Emoji under the SIL Open Font License, both in
`fonts/`.

## Contributing

Contributions are welcome. Whether it is code, documentation, or feedback, your
input helps make computing more accessible for everyone.
