# Changelog

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
