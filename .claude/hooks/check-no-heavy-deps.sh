#!/usr/bin/env bash
#
# Enforces the rule this crate exists for (see CLAUDE.md, "The boundary").
#
# sicompass-ui is linked into the login greeter as well as the application.
# Linking the application into a login screen cost 465 crates (wasmtime, a
# bundled SQLite, a headless-Chromium driver, an IMAP and an SMTP client), so
# nothing here may depend on `sicompass-builtins`, `sicompass-updater`,
# `wasmtime` or `reqwest`, nor on any `sicompass-<provider>` crate. Where the
# renderer needs an answer only the embedder has, it asks through
# `registry::HostHooks` or `http::register_body_fetcher`.
#
# Checked twice: in the manifest (a dependency added without a `use` yet) and
# in the source (a path reached through some other crate's re-export).
set -uo pipefail

ROOT="${CLAUDE_PROJECT_DIR:-.}"
status=0

FORBIDDEN='wasmtime|reqwest|sicompass-builtins|sicompass-updater|sicompass-(filebrowser|settings|chatclient|emailclient|webbrowser|tutorial|remote|sales-demo|shell|terminal|claude|gitclient|notes|project-management|payments|text-editor)'

hits=$(grep -nE "^[[:space:]]*($FORBIDDEN)[[:space:]]*=" "$ROOT/Cargo.toml" 2>/dev/null)
if [ -n "$hits" ]; then
  echo "sicompass-ui must not depend on the application's heavy crates or on a provider crate:" >&2
  echo "$hits" >&2
  status=2
fi

SRC_FORBIDDEN=$(echo "$FORBIDDEN" | tr '-' '_')
hits=$(grep -rnE "^use ($SRC_FORBIDDEN)\b|[^a-z_]($SRC_FORBIDDEN)::" "$ROOT/src" 2>/dev/null \
  | grep -vE '^[^:]+:[0-9]+:\s*//')
if [ -n "$hits" ]; then
  echo "sicompass-ui source references a crate it must not use. Ask the embedder" >&2
  echo "through registry::HostHooks or http::register_body_fetcher instead:" >&2
  echo "$hits" >&2
  status=2
fi

exit $status
