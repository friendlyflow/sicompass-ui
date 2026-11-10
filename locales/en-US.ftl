# Keyboard-hint labels — English (source/fallback). Each entry preserves the
# chord prefix from the source SHORTCUTS table; non-English locales also
# keep the chord literal (keys names don't translate) and only swap the verb.

hint-esc-back = Esc    Back
hint-esc-close = Esc    Close
hint-esc-folders = Esc    Folders

hint-up-previous = Up     Previous
hint-up-prev-match = Up     Prev match

hint-down-next = Down   Next
hint-down-next-match = Down   Next match

hint-right-open = Right  Open
hint-right-navigate = Right  Navigate
hint-right-cursor-right = Right  Cursor right

hint-left-back = Left   Back
hint-left-navigate = Left   Navigate
hint-left-cursor-left = Left   Cursor left

hint-pgup-page-up = PgUp   Page up
hint-pgdn-page-dn = PgDn   Page dn

hint-home-first = Home   First
hint-end-last = End    Last

hint-shift-up-select-up = Shift+Up Select up
hint-shift-down-select-dn = Shift+Down Select dn
hint-shift-left-select-left = Shift+Left Select left
hint-shift-right-select-right = Shift+Right Select right
hint-shift-home-sel-start = Shift+Home Sel. start
hint-shift-end-sel-end = Shift+End  Sel. end

hint-ctrl-home-line-start = Ctrl+Home Line start
hint-ctrl-end-line-end = Ctrl+End  Line end

hint-enter-activate = Enter  Activate
hint-enter-toggle = Enter  Toggle
hint-enter-confirm = Enter  Confirm
hint-enter-execute = Enter  Execute
hint-enter-append = Enter  Append
hint-enter-open = Enter  Open
hint-enter-follow-link = Enter  Follow link
hint-enter-go-to-element = Enter  Go to element
hint-enter-go-to-match = Enter  Go to match

hint-f-search = F      Search
hint-tab-manual = Tab    Manual
hint-tab-next-field = Tab    Next field

hint-bspc-backspace = Bspc   Backspace
hint-del-delete = Del    Delete
hint-del-delete-fwd = Del    Delete fwd
hint-f5-refresh = F5     Refresh

hint-m-meta = M      Meta
hint-z-timeline = Z      Timeline
hint-b-bookmark = b      Bookmark
hint-i-edit = I      Edit
hint-i-edit-input = I      Edit input
hint-a-append = A      Append
hint-d-dashboard = D      Dashboard
hint-s-scroll = S      Scroll
hint-command = :      Command
hint-shell = :      Shell
hint-claude = :      Claude
hint-repository = :      Repository

hint-ctrl-a-insert-after = Ctrl+A Insert after
hint-ctrl-a-select-all = Ctrl+A Select all
hint-ctrl-c-copy = Ctrl+C Copy
hint-ctrl-shift-c-copy-url-value = Ctrl+Shift+C Copy URL/value
hint-ctrl-d-delete = Ctrl+D Delete
hint-ctrl-enter-newline = Ctrl+Enter Newline
hint-ctrl-f-extended-search = Ctrl+F Extended search
hint-ctrl-i-insert-before = Ctrl+I Insert before
hint-ctrl-o-open = Ctrl+O Open
hint-ctrl-s-save = Ctrl+S Save
hint-ctrl-shift-a-insert-after = Ctrl+Shift+A Insert after
hint-ctrl-shift-i-insert-before = Ctrl+Shift+I Insert before
hint-ctrl-shift-s-save-as = Ctrl+Shift+S Save as
hint-ctrl-shift-tab-prev-tab = Ctrl+Shift+Tab Prev tab
hint-ctrl-shift-z-redo = Ctrl+Shift+Z Redo
hint-ctrl-t-new-tab = Ctrl+T New tab
hint-ctrl-tab-next-tab = Ctrl+Tab Next tab
hint-t-switch-tab = t      Switch tab
hint-enter-switch = Enter  Switch
hint-ctrl-u-install-update = Ctrl+U Install update
hint-ctrl-v-paste = Ctrl+V Paste
hint-ctrl-shift-t-close-tab = Ctrl+Shift+T Close tab
hint-ctrl-w-where-am-i = Ctrl+W Where am I
hint-del-close-tab = Del    Close tab
hint-ctrl-d-close-tab = Ctrl+D Close tab
hint-ctrl-x-cut = Ctrl+X Cut
hint-ctrl-z-undo = Ctrl+Z Undo

# Coordinate (input-mode) display names — spoken by the screen reader on
# every mode switch and shown in the header status line + window title.
mode-general = general mode
mode-insert = insert mode
mode-normal = normal mode
mode-visual = visual mode
mode-search = search mode
mode-extended-search = extended search mode
mode-command = command mode
mode-first-command = first command mode
mode-session = session mode
mode-second-command = second command mode
mode-controls = controls mode
mode-scroll = scroll mode
mode-scroll-search = scroll search mode
mode-input-search = input search mode
mode-dashboard = dashboard mode
mode-meta = meta mode
mode-timeline = timeline mode
mode-manual = beginner's manual mode
mode-tab-switcher = tab switcher mode
mode-confirm-close-tab = confirm close tab mode

# Screen-reader announcements composed at runtime. Parameters:
#   $idx, $total — 1-based position and tab count
#   $label       — translated provider name of the active tab
speak-tab-change = tab { $idx }/{ $total }: { $label }
# Spoken in the newly selected language when the UI language changes, so the
# screen reader switches voice even if the focused control's text is unchanged.
speak-language-change = Language changed

# Close-tab confirmation (the question above the buttons, also spoken) and its
# buttons. $name is the tab's breadcrumb. The -unnamed variants are used when
# the tab has no breadcrumb.
confirm-close-tab-prompt = Close tab "{ $name }"? This cannot be undone.
confirm-close-tab-prompt-unnamed = Close this tab? This cannot be undone.
confirm-close-tab-prompt-busy = RUNNING PROGRAM in tab "{ $name }". Closing it kills the program. This cannot be undone.
confirm-close-tab-prompt-busy-unnamed = RUNNING PROGRAM in this tab. Closing it kills the program. This cannot be undone.
confirm-close-tab-close = Close tab
confirm-close-tab-kill = Close tab and kill process
confirm-close-tab-cancel = Cancel
# First row of the t tab switcher, opens a new tab.
tab-switcher-new-tab = new tab
hint-c-controls = c      Controls
hint-insert = :      Insert
hint-ctrl-insert = Ctrl+: Insert
hint-enter-insert = Enter  Insert
insert-palette-empty = nothing to insert
escape-command-mode-first = press Escape to leave { $mode } first

# In element mode
mode-in-element = in element mode
hint-right-in-element = Right  In element
hint-enter-use-first = Enter  Use first
hint-esc-leave-element = Esc    Leave element
hint-right-next-target = Right  Next target
hint-left-previous-target = Left   Previous target
hint-up-line-above = Up     Line above
hint-down-line-below = Down   Line below
hint-d-view-image = D      View image
hint-enter-view-image = Enter  View image
hint-right-next-image = Right  Next image
hint-left-previous-image = Left   Previous image
target-link = link
target-image = image
target-button = button
target-position = { $kind } { $n } of { $total }, { $name }
target-followed = followed link, { $name }
target-pressed = pressed button, { $name }
targets-links = { $count ->
    [one] 1 link
   *[other] { $count } links
}
targets-images = { $count ->
    [one] 1 image
   *[other] { $count } images
}
targets-buttons = { $count ->
    [one] 1 button
   *[other] { $count } buttons
}
target-video = video
targets-videos = { $count ->
    [one] 1 video
   *[other] { $count } videos
}

hint-enter-pause = Enter  Pause
video-playing = playing
video-paused = paused
video-unavailable = This version of Sicompass cannot play video

search-no-matches = no matches

# The beginner's manual (Tab): one line each, read in order.
manual-01 = Beginner's manual. Press the Down arrow key now to hear the next line, and the Up arrow key to go back one line. Escape leaves the manual, and Tab opens it again.
manual-02 = Everything in Sicompass is a list, and you are in one now. Each line is one element, and your screen reader reads the line you are on. Read this manual from top to bottom.
manual-03 = Home jumps to the first line of a list and End to the last. PgDn and PgUp move a page at a time.
manual-04 = Right goes into a line that starts with an arrow right, and Left goes back out. The line above the list, with an arrow left, names the place you are in.
manual-05 = Press Home twice to reach the very top, where all your programs are listed. Open one, such as the file browser, notes or the store, with Right.
manual-06 = Every line starts with a mark. An arrow right means there is more inside. A dash means there is nothing inside. The letters after it say what kind of element it is, and the next lines go through them.
manual-07 = A dash with no letter is plain text. You can read it and that is all.
manual-08 = i is an input, a line of text you can change. Press i to start typing, or a to type at the end. Enter keeps what you typed, and Escape throws it away.
manual-09 = c is a checkbox. Press Enter to tick it or untick it. A letter after the c says what it holds, such as l for a link and b for a button. Press Right to reach it.
manual-10 = R is a group of choices, and r is one choice in it. Press Right to go into the group, move to a choice and press Enter. Only one can be picked. A letter after the r says what a choice holds, the same way. Press Right to reach it.
manual-11 = b is a button. Press Enter to run it. Sicompass tells you which button it was.
manual-12 = l is a link. Press Enter to follow it, and Left to come back. Right does nothing on a link. The line above the new list shows which link you followed.
manual-13 = A line can hold several links, buttons, pictures or videos, and then it starts with an arrow right. Enter uses the first of them. Press Right to move between them with the four arrows, Enter to use the one you are on, and Escape to leave.
manual-14 = p is a picture. Press Enter to see it big, and Escape to come back. v is a video. Press Enter to play it, and Enter again to pause it.
manual-15 = Now the modes. A mode is what the keys do at the moment, and the header at the top says which one you are in. Most of the time you are in general mode, where you move around and press Enter.
manual-16 = Insert mode: you enter it with i or a on an input. You type, Enter keeps the text, and Escape goes back to general mode.
manual-17 = Search mode: press f in general mode and type to narrow the list down. Enter goes to the match and Escape cancels. Ctrl+F searches everything below you, not only this list.
manual-18 = Command mode: press the colon key for the commands of the line you are on, such as open with. In the terminal it starts a shell.
manual-19 = Scroll mode: press s to read a long list as one page. PgUp, PgDn, Home and End move through it, and Escape leaves.
manual-20 = Element mode: press Right on a line with several links, buttons, pictures or videos. The arrows move between them, as the earlier lines showed. On a line with an arrow right, Right on the last one goes inside it.
manual-21 = Meta mode: press m to see every key that works on the line you are on. Timeline mode: press z to see what you did. Ctrl+Z undoes and Ctrl+Shift+Z redoes.
manual-22 = Beginner's manual mode is this list. Escape always takes you back to general mode, and Tab opens the manual again.
manual-23 = Tab switcher mode keeps several places open at once. Press t to open the switcher and pick one with Enter, or press Ctrl+T to open a new place.
manual-24 = Ctrl+C copies the line you are on, Ctrl+Shift+C copies a link or a value, and Ctrl+V pastes. Ctrl+W tells you where you are.
manual-25 = A screen reader reads every line you land on. If you are lost, press Escape, or press Home twice to start again from the top.
manual-26 = Next, open the tutorial from the list of programs. It lets you try every element in this manual. The settings program changes the font size and the colours.
