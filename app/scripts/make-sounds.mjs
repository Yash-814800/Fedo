// Renders Fedo's own sounds into app/sounds/*.wav.
//
//   node scripts/make-sounds.mjs
//
// Everything is synthesised from sine / triangle partials, short envelopes and
// filtered noise — no samples, nothing taken from anywhere (FEDO.md Phase 2,
// LICENSE-ASSETS.md). Mono, 16-bit, 32 kHz: they are short and soft, and the
// default master volume is 0.12, so peaks are kept well under full scale.
//
// File names are the names the app plays (src/core/sound.ts SOUND_NAMES). The
// FEDO.md names map as: done=finish, needs-approval=approval, poke=slap,
// timer-end=timer, listening-start=listenstart, listening-stop=listenstop,
// message-received=message. (Names are lowercase letters only: a user's own
// file in ~/.config/fedo/sounds/<name>.wav replaces one, and sounds.rs
// accepts nothing else.)

import { mkdirSync, writeFileSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";

const SR = 32000;
const OUT = join(dirname(fileURLToPath(import.meta.url)), "..", "sounds");
const TAU = Math.PI * 2;

// Deterministic noise so a re-run renders the same bytes.
let seed = 0x9e3779b9;
const rnd = () => {
  seed ^= seed << 13; seed >>>= 0;
  seed ^= seed >>> 17;
  seed ^= seed << 5; seed >>>= 0;
  return seed / 0xffffffff;
};

const buf = (sec) => new Float32Array(Math.max(1, Math.round(sec * SR)));
const mixInto = (dst, src, at = 0, gain = 1) => {
  const o = Math.round(at * SR);
  for (let i = 0; i < src.length && o + i < dst.length; i++) dst[o + i] += src[i] * gain;
};
const hz = (note) => 440 * Math.pow(2, (note - 69) / 12);
const N = { C4: 60, D4: 62, E4: 64, G4: 67, A4: 69, B4: 71, C5: 72, D5: 74, E5: 76, F5: 77, G5: 79, A5: 81, B5: 83, C6: 84, D6: 86, E6: 88, G6: 91, A6: 93 };

/**
 * One voice. `f0`→`f1` glides exponentially, `wave` is "sine" | "tri",
 * `bell` adds an inharmonic-ish 2.0x / 3.0x partial, `vib` is [rate, depth].
 */
function tone({ f0, f1 = f0, dur, wave = "sine", att = 0.006, rel = 0.6, bell = 0, vib = null, trem = null, amp = 1 }) {
  const out = buf(dur);
  let phase = 0;
  for (let i = 0; i < out.length; i++) {
    const t = i / SR;
    const k = t / dur;
    let f = f0 * Math.pow(f1 / f0, k);
    if (vib) f *= 1 + vib[1] * Math.sin(TAU * vib[0] * t);
    phase += (TAU * f) / SR;
    let v = wave === "tri" ? (2 / Math.PI) * Math.asin(Math.sin(phase)) : Math.sin(phase);
    if (bell) v += bell * 0.5 * Math.sin(2 * phase) + bell * 0.25 * Math.sin(3.01 * phase);
    // attack ramp, then an exponential-ish decay that reaches ~0 at the end
    const a = Math.min(1, t / att);
    const d = Math.pow(1 - k, 1 + rel * 2);
    let e = a * d;
    if (trem) e *= 1 - trem[1] * (0.5 + 0.5 * Math.sin(TAU * trem[0] * t));
    out[i] = v * e * amp;
  }
  return out;
}

/** Filtered noise whose one-pole low-pass sweeps c0→c1 (0..1). A soft whoosh. */
function whoosh({ dur, c0, c1, amp = 1, peak = 0.5 }) {
  const out = buf(dur);
  let y = 0;
  for (let i = 0; i < out.length; i++) {
    const k = i / out.length;
    const c = c0 + (c1 - c0) * k;
    y += c * (rnd() * 2 - 1 - y);
    // swell to `peak` of the length, then fall
    const e = k < peak ? Math.sin((k / peak) * Math.PI / 2) : Math.cos(((k - peak) / (1 - peak)) * Math.PI / 2);
    out[i] = y * e * amp * 3;
  }
  return out;
}

/** A sequence of [note-or-Hz-pair, startSeconds, options]. */
function seq(total, notes) {
  const out = buf(total);
  for (const [spec, at, o = {}] of notes) mixInto(out, tone({ ...spec }), at, o.gain ?? 1);
  return out;
}

function finish(b, peak = 0.7) {
  // Gentle fade at both ends (no clicks), then normalise.
  const fi = Math.min(b.length, Math.round(0.002 * SR));
  const fo = Math.min(b.length, Math.round(0.01 * SR));
  for (let i = 0; i < fi; i++) b[i] *= i / fi;
  for (let i = 0; i < fo; i++) b[b.length - 1 - i] *= i / fo;
  let m = 0;
  for (const v of b) m = Math.max(m, Math.abs(v));
  const g = m > 0 ? peak / m : 1;
  return b.map((v) => v * g);
}

const n = (name, o = {}) => ({ f0: hz(N[name]), ...o });

// ── The sounds ───────────────────────────────────────────────────────────────

const SOUNDS = {
  // island
  peek: () => seq(0.28, [[n("E5", { dur: 0.1, rel: 0.5 }), 0], [n("A5", { dur: 0.18, rel: 0.8, bell: 0.3 }), 0.07]]),
  open: () => {
    const b = buf(0.45);
    mixInto(b, whoosh({ dur: 0.28, c0: 0.04, c1: 0.3, amp: 0.5, peak: 0.7 }), 0, 1);
    mixInto(b, tone({ f0: hz(N.C6), dur: 0.32, rel: 1, bell: 0.35 }), 0.14, 0.7);
    return b;
  },
  close: () => {
    const b = buf(0.4);
    mixInto(b, whoosh({ dur: 0.26, c0: 0.3, c1: 0.04, amp: 0.5, peak: 0.25 }), 0, 1);
    mixInto(b, tone({ f0: hz(N.E5), f1: hz(N.C5), dur: 0.2, rel: 1 }), 0.1, 0.6);
    return b;
  },
  hover: () => tone({ f0: 1250, f1: 1500, dur: 0.035, rel: 0.2 }),
  blip: () => tone({ f0: 880, f1: 940, dur: 0.06, rel: 0.4 }),
  tick: () => tone({ f0: 1700, dur: 0.02, rel: 0.1, att: 0.002 }),
  pop: () => tone({ f0: 760, f1: 240, dur: 0.07, rel: 0.2, att: 0.002 }),
  send: () => tone({ f0: 520, f1: 1040, dur: 0.09, rel: 0.6 }),
  attach: () => seq(0.14, [[{ f0: 620, dur: 0.04, rel: 0.2, att: 0.002 }, 0], [{ f0: 930, dur: 0.06, rel: 0.4, att: 0.002 }, 0.06]]),
  approve: () => seq(0.2, [[n("E5", { dur: 0.07, rel: 0.5 }), 0], [n("B5", { dur: 0.13, rel: 0.9, bell: 0.25 }), 0.07]]),
  gulp: () => seq(0.3, [[{ f0: 320, f1: 130, dur: 0.1, rel: 0.5 }, 0], [{ f0: 240, f1: 95, dur: 0.14, rel: 0.7 }, 0.12]]),

  // poking the ghost
  slap: () => {
    const b = tone({ f0: 520, f1: 170, dur: 0.11, rel: 0.5, att: 0.003 });
    mixInto(b, whoosh({ dur: 0.05, c0: 0.5, c1: 0.2, amp: 0.2, peak: 0.1 }), 0, 1);
    return b;
  },
  annoyed: () => seq(0.3, [[n("E4", { dur: 0.12, wave: "tri", rel: 0.5 }), 0], [n("D4", { dur: 0.16, wave: "tri", f1: hz(N.C4), rel: 0.8 }), 0.11]]),
  dizzy: () => tone({ f0: 620, f1: 280, dur: 0.75, vib: [9, 0.07], rel: 0.4, att: 0.02 }),
  wink: () => seq(0.14, [[{ f0: 1200, f1: 1500, dur: 0.05, rel: 0.4 }, 0], [{ f0: 1800, dur: 0.07, rel: 0.7 }, 0.05]]),
  love: () => seq(0.5, [[n("G5", { dur: 0.22, rel: 0.8, vib: [6, 0.01], bell: 0.2 }), 0], [n("C6", { dur: 0.32, rel: 1, vib: [6, 0.01], bell: 0.3 }), 0.14]]),
  proud: () => seq(0.6, [[n("C5", { dur: 0.14, wave: "tri" }), 0], [n("G5", { dur: 0.14, wave: "tri" }), 0.12], [n("C6", { dur: 0.36, wave: "tri", rel: 1, bell: 0.2 }), 0.24]]),
  yawn: () => tone({ f0: 520, f1: 230, dur: 0.7, vib: [5, 0.02], att: 0.12, rel: 0.5, amp: 0.8 }),
  greet: () => seq(0.5, [[n("C5", { dur: 0.12, wave: "tri" }), 0], [n("E5", { dur: 0.12, wave: "tri" }), 0.09], [n("G5", { dur: 0.26, wave: "tri", rel: 1 }), 0.18]]),
  greeting: () => {
    const b = buf(1.4);
    for (const [name, g] of [["C4", 1], ["E4", 0.8], ["G4", 0.7], ["C5", 0.4]]) {
      const t = tone({ f0: hz(N[name]), dur: 1.4, att: 0.45, rel: 0.4, amp: g });
      mixInto(b, t, 0, 0.5);
    }
    return b;
  },

  // states
  work: () => seq(0.2, [[{ f0: 520, dur: 0.07, rel: 0.4 }, 0], [{ f0: 660, dur: 0.09, rel: 0.6 }, 0.08]]),
  think: () => tone({ f0: 440, f1: 470, dur: 0.45, att: 0.05, trem: [11, 0.5], rel: 0.4, amp: 0.8 }),
  search: () => tone({ f0: 420, f1: 840, dur: 0.4, att: 0.03, vib: [4, 0.03], rel: 0.3, amp: 0.8 }),
  finish: () => seq(0.85, [[n("C6", { dur: 0.25, rel: 1, bell: 0.4 }), 0], [n("E6", { dur: 0.25, rel: 1, bell: 0.4 }), 0.11], [n("G6", { dur: 0.5, rel: 1.2, bell: 0.4 }), 0.22]]),
  error: () => seq(0.4, [[{ f0: 233, f1: 200, dur: 0.14, wave: "tri", rel: 0.4 }, 0], [{ f0: 196, f1: 165, dur: 0.22, wave: "tri", rel: 0.8 }, 0.16]]),
  approval: () => seq(0.55, [[n("A5", { dur: 0.22, rel: 1, bell: 0.4 }), 0], [n("A5", { dur: 0.3, rel: 1.1, bell: 0.4 }), 0.24, { gain: 0.8 }]]),
  question: () => seq(0.4, [[{ f0: 420, f1: 640, dur: 0.14, rel: 0.5 }, 0], [{ f0: 560, f1: 780, dur: 0.2, rel: 0.9, bell: 0.2 }, 0.14]]),
  rate: () => seq(0.5, [[n("E5", { dur: 0.2, wave: "tri", rel: 0.7 }), 0], [n("C5", { dur: 0.28, wave: "tri", rel: 1 }), 0.2]]),
  sleep: () => seq(0.9, [[n("E5", { dur: 0.3, rel: 1, att: 0.04, amp: 0.8 }), 0], [n("C5", { dur: 0.3, rel: 1, att: 0.04, amp: 0.8 }), 0.28], [n("A4", { dur: 0.45, rel: 1.2, att: 0.04, amp: 0.8 }), 0.56]]),

  // new in Phase 2: timers, reminders, messages, the mic
  timer: () => {
    const hit = (at) => [n("E6", { dur: 0.16, rel: 1, bell: 0.5 }), at];
    return seq(1.0, [hit(0), hit(0.2), hit(0.4), [n("E6", { dur: 0.45, rel: 1.2, bell: 0.5 }), 0.7]]);
  },
  reminder: () => seq(0.8, [[n("G5", { dur: 0.35, rel: 1, bell: 0.45 }), 0], [n("D5", { dur: 0.5, rel: 1.2, bell: 0.45 }), 0.26]]),
  message: () => seq(0.35, [[n("D6", { dur: 0.1, rel: 0.6, bell: 0.2 }), 0], [n("A6", { dur: 0.2, rel: 1, bell: 0.3 }), 0.08]]),
  listenstart: () => seq(0.22, [[{ f0: 600, f1: 760, dur: 0.08, rel: 0.4 }, 0], [{ f0: 900, dur: 0.12, rel: 0.8, bell: 0.2 }, 0.08]]),
  listenstop: () => seq(0.22, [[{ f0: 900, f1: 760, dur: 0.08, rel: 0.4 }, 0], [{ f0: 600, dur: 0.12, rel: 0.8, bell: 0.2 }, 0.08]]),
};

// Per-sound level, so a short tick doesn't sit as loud as a chime.
const PEAK = { hover: 0.35, tick: 0.35, blip: 0.5, greeting: 0.6, sleep: 0.55, think: 0.5, search: 0.5 };

function wav(samples) {
  const data = Buffer.alloc(samples.length * 2);
  for (let i = 0; i < samples.length; i++) {
    const v = Math.max(-1, Math.min(1, samples[i]));
    data.writeInt16LE(Math.round(v * 32767), i * 2);
  }
  const head = Buffer.alloc(44);
  head.write("RIFF", 0);
  head.writeUInt32LE(36 + data.length, 4);
  head.write("WAVEfmt ", 8);
  head.writeUInt32LE(16, 16);
  head.writeUInt16LE(1, 20); // PCM
  head.writeUInt16LE(1, 22); // mono
  head.writeUInt32LE(SR, 24);
  head.writeUInt32LE(SR * 2, 28);
  head.writeUInt16LE(2, 32);
  head.writeUInt16LE(16, 34);
  head.write("data", 36);
  head.writeUInt32LE(data.length, 40);
  return Buffer.concat([head, data]);
}

mkdirSync(OUT, { recursive: true });
let total = 0;
for (const [name, make] of Object.entries(SOUNDS)) {
  seed = 0x9e3779b9; // each sound renders the same whatever the order
  const file = wav(finish(make(), PEAK[name] ?? 0.7));
  writeFileSync(join(OUT, `${name}.wav`), file);
  total += file.length;
}
console.log(`${Object.keys(SOUNDS).length} sounds, ${(total / 1024).toFixed(0)} KiB → ${OUT}`);
