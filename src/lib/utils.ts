import { createCn } from "cn/config";

// shadcn's `cn` with our theme scales (theme.css). Without them `text-body` reads as a color and
// `cn("text-body", "text-ink")` drops the size, and two of our widths or keyframes both stay.
// Components import this, never "cn" itself.
export const cn = createCn({
  extend: {
    theme: {
      text: ["display-xl", "display-l", "display-m", "title", "body-doc", "body", "label", "caption"],
      radius: ["control", "float"],
      shadow: ["float"],
      animate: ["breathe", "bar", "dot", "shimmer", "ring"],
      container: ["doc", "home", "wide"],
    },
  },
});

/** The particle after `word`: `withFinal` after a Hangul syllable with a final consonant, else
 * `without`, also after anything that is not Hangul ("#dev로"). "으로" takes "로" after ㄹ too
 * ("#개발로"). Instead of "(으)로" in the words we show. */
export function josa(word: string, withFinal: string, without: string) {
  const syllable = word.charCodeAt(word.length - 1) - 0xac00;
  if (!(syllable >= 0 && syllable < 11172)) return without;
  const final = syllable % 28;
  // 8 is ㄹ.
  return final && !(final === 8 && withFinal === "으로") ? withFinal : without;
}
