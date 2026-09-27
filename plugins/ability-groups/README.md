# Ability groups

Keeps an ability button when the selected squads have more than one ability
for it.

The four ability buttons above the order panel (F1–F4) each hold several
abilities: F2 holds mines and C4, for example. The game shows one ability per
button, and when a selection has two abilities of the same button it shows
neither, and the key does nothing until the selection changes. Engineers
selected with guerillas lose F2 this way.

With this plugin the button shows one of them, and pressing it (its key or a
click) opens a row of all of them over the order panel, the way the mine
button opens its mine types. Pick one with the order key shown above it (Q, R,
F or G with the default keys, following your key settings) or with the mouse.
The button then shows the ability you picked last. Press the ability key
again, press another ability's key, click another button, right-click or
press Escape to close the row without picking; changing the selection closes
it too.

Settings, under `[defiance.ability-groups]` in
`DefianceLoader/config/infantry.ini`:

- `enabled` (on).

Experimental; checked in game with engineers and C4 rangers. Single-player: like the other plugins
that ship with the loader, it blocks online multiplayer while active.
