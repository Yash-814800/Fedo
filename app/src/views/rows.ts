// A keyboard-walkable list inside the overview's left card: Ctrl+↓ / Ctrl+↑
// move the highlight (State.cardSelection), Ctrl+O does what a click on the
// row does. Fedo has no card with such a list yet — the GitHub pulse that used
// it came out in Phase 0 — but the key handling is kept for the ones to come
// (Phase 6's messages, the deadlines). A list is a `.card-list` element whose
// children are its rows.

const rowOpens = new WeakMap<HTMLElement, () => void>();

/** Makes `el` a row of a card list: `open` is what click / Ctrl+O does. */
export function makeRow(el: HTMLElement, open: () => void): HTMLElement {
  rowOpens.set(el, open);
  el.addEventListener("click", open);
  return el;
}

/** The rows of the list shown in `root`, top to bottom; none without one. */
export function listRows(root: HTMLElement): HTMLElement[] {
  const list = root.querySelector(".card-list");
  return list ? (Array.from(list.children) as HTMLElement[]) : [];
}

/** Highlights row `selection` and no other; `reveal` scrolls it into view. */
export function highlightRow(rows: readonly HTMLElement[], selection: number | null, reveal: boolean) {
  rows.forEach((el, i) => el.classList.toggle("selected", i === selection));
  if (reveal && selection != null) rows[selection]?.scrollIntoView({ block: "nearest" });
}

/** Ctrl+O: does what a click on the row does. */
export function openRow(el: HTMLElement | undefined) {
  if (el) rowOpens.get(el)?.();
}
