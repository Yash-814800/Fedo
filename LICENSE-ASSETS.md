# Fedo — name, character and sounds

Fedo is a personal fork of [coucou](https://github.com/Louis-CFM/coucou) by
Louis Raillé. coucou's own name, its "Mochi" character, its icon and its
sounds are rights-reserved assets that are **not part of this fork** — see
the previous rights holder's own terms at
[Louis-CFM/coucou](https://github.com/Louis-CFM/coucou/blob/main/LICENSE-ASSETS.md).

The [MIT License](LICENSE) covers Fedo's source code, including the code
that draws its own ghost character (there are no art files to license
separately: the character is drawn in code, `Canvas` + `TimelineView`-style,
per `CLAUDE.md`'s rules). Fedo's name, ghost design and sounds are this
fork's own and carry no additional restriction beyond the MIT License.

The sounds in `app/sounds/` are rendered by `app/scripts/make-sounds.mjs`
from sine/triangle oscillators and filtered noise — no samples — and are
covered by the same terms as the code.
