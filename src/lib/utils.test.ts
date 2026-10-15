import { expect, test } from "vitest";
import { cn, josa } from "./utils";

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

test("josa picks the particle from the last syllable", () => {
  const ro = (word: string) => `${word}${josa(word, "으로", "로")}`;
  // A final consonant takes 으로; none, ㄹ, or not Hangul at all takes 로.
  expect(["#회의록", "#채용", "#회의", "#개발", "회사 클라우드", "Ollama", "#dev", "GPT-4", ""].map(ro)).toEqual([
    "#회의록으로",
    "#채용으로",
    "#회의로",
    "#개발로",
    "회사 클라우드로",
    "Ollama로",
    "#dev로",
    "GPT-4로",
    "로",
  ]);
  // Other pairs count ㄹ as a final consonant.
  expect(josa("모델", "을", "를")).toBe("을");
  expect(josa("회의", "을", "를")).toBe("를");
  expect(josa("파일", "이", "가")).toBe("이");
  expect(josa("웹훅", "이", "가")).toBe("이");
  expect(josa("키", "이", "가")).toBe("가");
});
