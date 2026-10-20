import { mockWindows } from "@tauri-apps/api/mocks";
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
// No layout: a range has no box (the meeting document reads the caret's off the editor's mirror).
Range.prototype.getBoundingClientRect ??= () => new DOMRect();

// The window and webview App asks for (`getCurrentWebview`, file drops). Each test's `mockIPC`
// keeps this.
mockWindows("main");

// Motion finishes every animation at once, so tests see the end state and exits leave at once.
MotionGlobalConfig.skipAnimations = true;
