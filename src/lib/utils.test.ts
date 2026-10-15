import { expect, test } from "vitest";
import { cn } from "./utils";

test("cn knows the theme's text sizes, radii and shadow", () => {
  // A color after a size keeps the size; a later size replaces it.
  expect(cn("text-body text-ink", "text-ink-2")).toBe("text-body text-ink-2");
  expect(cn("text-body", "text-label")).toBe("text-label");
  expect(cn("rounded-control", "rounded-md")).toBe("rounded-md");
  expect(cn("shadow-float", "shadow-none")).toBe("shadow-none");
  expect(cn("motion-safe:animate-breathe", "motion-safe:animate-none")).toBe("motion-safe:animate-none");
  expect(cn("max-w-doc", "max-w-home")).toBe("max-w-home");
});

// `shadcn add` writes `from "cn"`, which knows none of the theme's sizes.
test("components use the configured cn", () => {
  const sources = import.meta.glob<string>("../components/**/*.tsx", {
    query: "?raw",
    import: "default",
    eager: true,
  });
  const plain = Object.keys(sources).filter((file) => /from ["']cn["']/.test(sources[file]));
  expect(plain).toEqual([]);
});
