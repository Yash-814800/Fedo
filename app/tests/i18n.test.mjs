// Interface language (src/i18n): English only for now (FEDO.md Phase 0), but
// the mechanism stays so Hindi can be added back without a rewrite — so these
// check the mechanism: lookup, fallback, interpolation, plurals, live labels.

import { test } from "node:test";
import assert from "node:assert/strict";
import { installFakeDom } from "./fakedom.mjs";

installFakeDom();

const {
  LANGUAGES, LANGUAGE_CODES, N_, dayMonth, isRtl, labels, language, lookup, monthShort,
  resolveLanguage, setLanguage, t, tl, tn, weekdayShort,
} = await import("../src/i18n/i18n.ts");
const { h, relabel, liveTextCount } = await import("../src/views/dom.ts");

test("English is the only language, and it is the key itself", () => {
  assert.deepEqual([...LANGUAGE_CODES], ["en"]);
  assert.deepEqual(LANGUAGES.map((l) => l.code), ["en"]);
  assert.equal(language(), "en");
  assert.equal(t("Allow"), "Allow");
  assert.equal(lookup("Allow"), undefined, "nothing is stored for English");
  assert.equal(isRtl(), false);
});

test("any other language request falls back to English", () => {
  assert.equal(resolveLanguage("fr"), "en");
  assert.equal(resolveLanguage(undefined, ["hi-IN", "de"]), "en");
  assert.equal(resolveLanguage("en"), "en");
  assert.equal(setLanguage("en"), false, "already English: nothing changes");
});

test("placeholders are filled in, and unknown ones are left as they are", () => {
  assert.equal(t("Uploading {name}", { name: "a.pdf" }), "Uploading a.pdf");
  assert.equal(t("Uploading {name}"), "Uploading {name}");
  assert.equal(t("in {h} h {m}", { h: 1, m: 20 }), "in 1 h 20");
});

test("plurals pick the English form by count", () => {
  assert.equal(tn("{count} file", "{count} files", 1), "1 file");
  assert.equal(tn("{count} file", "{count} files", 3), "3 files");
  assert.equal(tn("{count} file", "{count} files", 0), "0 files");
});

test("N_ marks a key and labels maps a table of them", () => {
  assert.equal(N_("Cancel"), "Cancel");
  assert.deepEqual(labels({ a: "Allow", b: "Deny" }), { a: "Allow", b: "Deny" });
});

test("dates read in English", () => {
  assert.equal(monthShort(9), "Oct"); // 0-based, like Date.getMonth()
  assert.equal(weekdayShort(5), "Fri"); // 0 = Sunday, like Date.getDay()
  assert.match(dayMonth(9, 16), /16.*Oct|Oct.*16/);
});

test("a lazy label becomes text, and a bound element keeps it on relabel", () => {
  const el = h("button", { title: tl("Allow") }, tl("Deny"));
  assert.equal(el.textContent, "Deny");
  assert.equal(el.getAttribute("title"), "Allow");
  const before = liveTextCount();
  relabel();
  assert.equal(liveTextCount(), before);
  assert.equal(el.textContent, "Deny");
});
