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
      animate: ["breathe", "bar", "dot", "shimmer"],
      container: ["doc", "home", "wide"],
    },
  },
});
