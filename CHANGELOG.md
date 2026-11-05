# Changelog

## 0.2.2

Needs sicompass-sdk 0.9.13.

- Pictures, grids and video in a row. `boxes.rs` lays out CSS grid and flex with
  taffy, a row can hold several pictures or a video, and `video.rs` plays a
  `<video>` in the window through a decoder the embedder registers. Enter pauses
  and resumes it. One layout pass serves every list (`layout.rs`), real CSS
  styles them (`style.rs`: selectors, cascade, `user.css`), and every background
  is rounded by default.
- In element mode (`i` or `a`) moves between a row's pictures, links and buttons.
  Enter activates and says which one it used, and Enter or `d` on a picture opens
  a big view.
- Links are a dash `l`, followed with Enter, and Right does nothing on one. The
  page that opens shows the followed link on its parent line. Links and buttons
  stand out in every row, and the cursor's row is a rounded border.
- List prefixes are Bootstrap icons: an arrow right and a dash followed by their
  letters, and a back arrow on the parent line. A row holding a video says `v`.
- Checkboxes and radio choices change with Enter whatever they hold, and `i` or
  `a` reach a link or button in them. Their prefix says what they hold (`-cl`,
  `-rcb`), they keep it when selected, and they draw their box in a row of boxes.
- Line numbers for a file (`AppRenderer::line_numbers`: none, absolute, relative)
  in a right-aligned column at the end of the list prefix.
- F is the search and Tab opens the beginner's manual, in four languages.
- Scroll mode shows only content, with deeper levels indented.
- Speech, confirmed with Orca: moves between similar rows are spoken, a typed key
  is said once, a search key is said in front of its row, and a paste is said.
- Fixed: Ctrl+F results no longer run over the last letter of the prefix, and tabs
  restore by folder name instead of saved row numbers.
- Intel macs are not built any more.

## 0.2.1

Needs sicompass-sdk 0.9.6 (plugin protocol 1.2).

- Scroll mode fetches the levels the user has not opened, breadth first and
  capped (4 levels, 5000 rows, 500 ms). It skips links, meta, session views and
  providers that opt out (`Provider::allows_scroll_prefetch`). S works in an
  editor too.
- An edit the provider refuses with a reason stays open and shows the reason in
  the header, instead of showing as saved. That covers a refused create, delete,
  rename, paste or save. Keys that open a row (Ctrl+A, Ctrl+I, i, a) ask the
  provider first (`cannot_add_here`).
- A button that opens a session lands on its prompt row, and a session with no
  name takes its name from its first prompt.
- Launcher mode (`launcher_window`): Escape in General dismisses the window, and
  there are no tabs, undo, redo or timeline keys. `DELTA_MS` is public, so an
  embedder can use the same double-tap window.
- One machine-wide accessibility object, shared with the greeter, and a screen
  reader the greeter can start. Checkboxes and radios work in launcher mode.
- Screen readers: Orca is ready when it reports READY=1, it starts in its own
  session, a newly started Orca is handed the focused row, a late screen reader
  hears cursor moves, and its quitting is reported.
- Password rows have the password role only while a password is typed, and the
  embedder can reveal the password being typed (`password_revealed`). The window
  is named after the app.
- Provider errors stay on screen.
