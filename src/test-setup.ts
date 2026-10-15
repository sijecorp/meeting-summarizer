import { MotionGlobalConfig } from "motion/react";

// jsdom lacks browser APIs that Radix (Dialog, Select, Switch) calls (architecture 12.1).
globalThis.ResizeObserver ??= class {
  observe() {}
  unobserve() {}
  disconnect() {}
};
Element.prototype.hasPointerCapture ??= () => false;
Element.prototype.releasePointerCapture ??= () => {};
Element.prototype.scrollIntoView ??= () => {};

// Motion finishes every animation at once, so tests see the end state and exits leave at once.
MotionGlobalConfig.skipAnimations = true;
