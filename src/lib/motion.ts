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

/** Between list rows appearing. */
export const stagger = 0.035;

/** Screens. `custom` is the direction: 1 forward (a new screen rises from 8px below, the old one
 * drifts up), -1 back to Home (the reverse). */
export const screenVariants: Variants = {
  enter: (dir: number) => ({ opacity: 0, y: 8 * dir }),
  show: { opacity: 1, y: 0, transition: { duration: dur.medium, ease: ease.out } },
  exit: (dir: number) => ({
    opacity: 0,
    y: -8 * dir,
    transition: { duration: exitDur(dur.medium), ease: ease.in },
  }),
};

/** App banners: they fade in place while `layout` moves what is around them. */
export const bannerVariants: Variants = {
  enter: { opacity: 0, y: -4 },
  show: { opacity: 1, y: 0, transition: { duration: dur.small, ease: ease.out } },
  exit: { opacity: 0, transition: { duration: exitDur(dur.small), ease: ease.in } },
};
