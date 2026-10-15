import type { Transition, Variants } from "motion/react";

// Motion tokens (design-system 5): the one place springs, durations and curves live. Only
// `transform` and `opacity` move; a changing height is a `layout` animation.

export const spring = {
  /** Presses, switches, tab indicators. */
  snappy: { type: "spring", stiffness: 520, damping: 38 },
  /** Layout moves and shared elements. */
  gentle: { type: "spring", stiffness: 260, damping: 30 },
  /** The recording dock appearing and changing shape. */
  dock: { type: "spring", stiffness: 380, damping: 28, mass: 0.9 },
} satisfies Record<string, Transition>;

/** Seconds, as motion takes them: 120 / 200 / 320 / 480 ms. */
export const dur = { micro: 0.12, small: 0.2, medium: 0.32, large: 0.48 };

export const ease = {
  out: [0.22, 1, 0.36, 1],
  in: [0.55, 0, 1, 0.45],
} satisfies Record<string, [number, number, number, number]>;

/** An exit takes 65% of its entrance. */
export const exitDur = (enter: number) => enter * 0.65;

/** A screen change: `dir` 1 forward from Home (the new screen rises from 8px below, the old one
 * drifts up), -1 back to Home (the reverse). `instant` when a shortcut caused it: actions from the
 * keyboard do not move (design-system 5). */
export type ScreenMove = { dir: 1 | -1; instant: boolean };
const now = { duration: 0 };

/** Screens, with a `ScreenMove` as `custom`. */
export const screenVariants: Variants = {
  enter: ({ dir }: ScreenMove) => ({ opacity: 0, y: 8 * dir }),
  show: ({ instant }: ScreenMove) => ({
    opacity: 1,
    y: 0,
    transition: instant ? now : { duration: dur.medium, ease: ease.out },
  }),
  exit: ({ dir, instant }: ScreenMove) => ({
    opacity: 0,
    y: -8 * dir,
    transition: instant ? now : { duration: exitDur(dur.medium), ease: ease.in },
  }),
};

/** A list whose rows rise in one after another, 35ms apart. Only the first eight wait their turn
 * (280ms): a long list must not keep its last rows out for seconds (300ms on daily screens). */
export const listVariants: Variants = {
  hidden: {},
  show: { transition: { delayChildren: (i: number) => Math.min(i, 8) * 0.035 } },
};
export const rowVariants: Variants = {
  hidden: { opacity: 0, y: 6 },
  show: { opacity: 1, y: 0, transition: { duration: dur.small, ease: ease.out } },
};

/** The summary arriving in its document (design-system 6.6): section by section, 40ms apart with
 * the list's cap, once. A textarea cannot move its text, so a read-only layer does it. */
export const revealVariants: Variants = {
  hidden: {},
  show: { transition: { delayChildren: (i: number) => Math.min(i, 8) * 0.04 } },
};
export const sectionVariants: Variants = {
  hidden: { opacity: 0, y: 4 },
  show: { opacity: 1, y: 0, transition: { duration: dur.small, ease: ease.out } },
};

/** A meeting's title gliding between its Home row and its document (design-system 6.1). A tween,
 * not `spring.gentle`: daily screens keep UI motion within 300ms. */
export const titleMove = { duration: 0.3, ease: ease.out };
/** How long after a screen change a title may glide: the glide and some slack. Later moves of the
 * same title (a row changing group, a meta line wrapping) are not a glide. */
export const TITLE_GLIDE_MS = 600;

/** The summary's document and its editor taking each other's place (design-system 6.6): a 150ms
 * crossfade, or none when a key did it (`custom`). */
export const swapVariants: Variants = {
  out: (instant: boolean) => ({ opacity: 0, transition: instant ? now : { duration: 0.15 } }),
  in: (instant: boolean) => ({ opacity: 1, transition: instant ? now : { duration: 0.15 } }),
};

/** App banners: they fade in place while `layout` moves what is around them. */
export const bannerVariants: Variants = {
  enter: { opacity: 0, y: -4 },
  show: { opacity: 1, y: 0, transition: { duration: dur.small, ease: ease.out } },
  exit: { opacity: 0, transition: { duration: exitDur(dur.small), ease: ease.in } },
};
