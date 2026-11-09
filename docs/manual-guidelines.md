# Beginner's Manual Guidelines

How to write and extend the beginner's manual, the list Tab opens in general mode
(`Coordinate::Manual`).

These are the rules we hold the manual to. The tutorial in the sicompass repo has its
own rules (`docs/tutorial-guidelines.md` there), and the two texts are kept up to date
the same way. Read this before you add, change or reorder a line, and check new lines
against it.

## What the manual is, and what the tutorial is

- **The manual** is short and linear. It is one list with no parent, read from the
  top with Down, one line at a time. It answers "what do the keys do" for someone who
  has never used Sicompass, and it works in every program that uses this renderer,
  including the login screen.
- **The tutorial** is hands-on and lives in the application only. It is a tree the
  learner walks, with a playground to try every element, the full shortcut reference,
  and a section for each program.

The manual points to the tutorial for practice, and the tutorial does not repeat the
manual. Neither should grow into the other.

## Where the lines live

- The application's manual is `manual-01` to `manual-NN` in this crate's
  `locales/<lang>.ftl`, with `NN` equal to `MANUAL_LINES` in `src/list.rs`.
- An embedder with its own manual registers its own keys, for example
  `login-manual-01` and on in loginsicompass, and sets
  `AppRenderer::manual = ManualText { prefix, lines }`. Write it when the app's lines
  speak of things that embedder does not have, such as programs, tabs, the Store or
  the tutorial.
- The list is built by `list::build_manual_list` and reads `ManualText::line`.

## The rules

### 1. It is read from the top, line by line

The reader presses Down and hears the next line. A line may only rely on what an
earlier line taught. Put the order first: reading the manual, then moving, then the
marks at the start of a line, then each kind of element, then the modes, then where
to go next.

### 2. One idea per line

A line says one thing, in one to three short sentences. If it needs more, split it
into two lines.

### 3. A line about a key starts with the key

Screen-reader users skim by the first words. "Home jumps to the first line" and
"i is an input" put the key or the mark first. A line about a mode starts with the
mode's name: "Search mode: press f ...".

### 4. Only what this program has

Every line must be true where it is read. The application's manual can send the
reader to the programs list, the Store and the tutorial. The login screen's cannot,
so it has its own lines. When a key does nothing in an embedder (tabs in the login
screen, for example), its manual does not mention it.

### 5. Say what is spoken

The screen reader is the only feedback many readers get. When a line describes an
action, say what is announced or shown afterwards ("Sicompass tells you which button
it was"), so the reader can recognize success.

### 6. Prose

No em dashes and no semicolons, in any language. Use commas, or two sentences.
Parentheses are fine for a true aside. Punctuation becomes a pause in speech, and
these two read oddly.

### 7. Every line in every language

A line exists in all four locale files (`en-US`, `nl-BE`, `fr-BE`, `de-BE`), with the
same number. A missing key shows as an empty row.

## When to update it

Update the manual, in the same change, whenever any of these change:

- what a key does, or which mode it works in
- a mode, its name, or how it is entered or left
- an element type, its mark in the list prefix (`-c`, `-cl`, `-l`, `-v` and so on), or
  how it is used
- what is said or shown after an action the manual describes

Check the tutorial in the same change: its shortcut and playground sections describe
the same keys. A soft hook in the sicompass repo (`.claude/hooks/check-help-texts.sh`)
reminds you when a change to the keys left both texts alone.

## When you add or change a line

1. Write the line in all four `.ftl` files, with the same number. Renumber the lines
   after it when you insert one, and keep the numbers two digits.
2. Set `MANUAL_LINES` (or the embedder's `ManualText::lines`) to the new count.
3. Follow the prose rule.
4. Run `cargo test`. `help_text::check_manual` fails on a missing or extra line, an
   empty line, an em dash or a semicolon. Embedders call it on their own files.
