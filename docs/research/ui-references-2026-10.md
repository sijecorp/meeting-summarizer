# Granola and editorial-UI research for the meeting app redesign (2026-10-15)

Three findings change the current plan in `docs/design-system.md`:

- **Bold Hangul titles will clash with light Latin.** Granola's serif display type is weight 400 only. Instrument Serif has only 400, so a 600-weight Noto Serif KR title puts heavy Hangul next to light Latin and digits.
- **`scaleY` bars warp their rounded ends.** Granola wrote a post about exactly this for their audio bars.
- **The list-to-document title morph changes font family and size,** and its target is an `<input>`, so it needs a specific recipe (C).

Things that could not be verified are flagged inline and listed at the end of E.

---

## A. Granola patterns

### Home and sidebar
- Home has two parts. "Coming up" shows upcoming calendar meetings, and clicking one creates its note. Below it is a list of all your notes. https://docs.granola.ai/help-center/getting-started/granola-101.md
- The sidebar holds only Spaces ("My notes" and the team space) and folders. As of Mar 2026, folders live inside Spaces. The meeting list itself is not in the sidebar. https://docs.granola.ai/help-center/getting-started/granola-101.md , https://releasebot.io/updates/granola
- Marketing screenshots show a "Today" group label with a count. https://www.granola.ai/
- Rows, not tiles: title, time, and attendees, with "+N" after about two names. Secondary source only. https://cdn.jsdelivr.net/npm/@agent-native/core@0.124.6/corpus/templates/clips/desktop/design-refs/granola-ux.md
- Jan 2026 polish: sidebar and notes contrast raised, sidebar icon animation fixed, navbar icons made consistent. https://www.granola.ai/updates/whats-new-2026-01-08
- Back from a note always returns to home or a folder, never to another meeting. That gives a clean rule for transition direction. https://www.granola.ai/blog/back-button

### Note view
- The title is edited inline. The default title changed from "Untitled meeting" to "New note". The share button stays hidden until a summary exists. The attendee card in the header was redesigned. https://www.granola.ai/updates/whats-new-2026-01-08
- The bottom bar has a "My notes / Enhanced" toggle, a "Transcribing" status, a "Generate notes" button, and an "Ask anything" input with prompt chips such as "List actions" and "Write follow-up email". https://www.granola.ai/
- Your own typed words are shown in black inside the enhanced notes. https://docs.granola.ai/help-center/taking-notes/ai-enhanced-notes.md
- Older docs and reviews say AI additions are grey. https://www.happyscribe.com/blog/granola-ai-review , https://cdn.jsdelivr.net/npm/@agent-native/core@0.124.6/corpus/templates/clips/desktop/design-refs/granola-ux.md
- Each summary bullet has a magnifying glass that shows its transcript source. A template picker sits top-right with a regenerate button. https://docs.granola.ai/help-center/taking-notes/ai-enhanced-notes.md , https://www.zapier.com/blog/granola-ai
- **Focus mode (Apr 2026):** the window chrome fades while you type and comes back when you stop. https://createwith.com/tool/granola/updates/granola-adds-focus-mode-to-reduce-distractions-while-taking-meeting-notes
- **Briefs (May 2026):** a short cited summary appears when you open a note and hides once read. https://releasebot.io/updates/granola

### Enhance flow
- Generation starts by itself when the call ends or recording stops. It uses the transcript, your notes, and the calendar event. It can be regenerated with another template. https://docs.granola.ai/help-center/taking-notes/ai-enhanced-notes.md
- It happens inside the note. There is no separate processing screen.
- Deleting a transcript chunk prompts you to regenerate. The Jan 2026 release added "smarter regeneration warnings". https://releasebot.io/updates/granola , https://www.granola.ai/updates
- **Not verified:** the enhance animation, and what the "generating" state looks like. One secondary source says "quiet, subtle progress, not a modal".

### Recording indicator
- Green animated bars sit at the bottom of the window. A waveform icon left of "Ask anything" shows or hides the transcript panel. https://docs.granola.ai/help-center/taking-notes/transcription.md
- When you are in another app, a floating indicator sits on the right screen edge. Clicking it returns to the note, and a bottom handle lets you drag it. Internally it is called "the nub". https://docs.granola.ai/help-center/taking-notes/transcription.md , https://www.granola.ai/blog/back-button
- The menu bar shows "10m left" during a meeting. https://www.granola.ai/blog/whats-new-2025-12-12
- Recording auto-stops when the call ends, after 15 minutes of silence, or when the computer sleeps. Ad-hoc meetings need a manual stop. https://docs.granola.ai/help-center/taking-notes/transcription.md
- Performance story:
  - The three "dancing bars" originally used `transition: height 300ms` with level updates about 10 times a second, and cost 60% CPU.
  - Switching to `scaleY` warped the rounded ends.
  - The final version makes each bar from two rounded caps moved with `translate`. Renderer CPU fell from 15% to 6% and GPU from 20% to under 1%.
  - https://www.granola.ai/blog/dont-animate-height
- Meeting pop-ups show one minute before a meeting, top-right, in Granola's own window. Calendar meetings get a solid left bar and ad-hoc calls get a dashed one, e.g. "Meeting detected" with a "Take Notes" button. https://docs.granola.ai/help-center/taking-notes/notifications.md

### Transcript
- Grey bubbles on the left are system audio ("Them"). Coloured bubbles on the right are the mic ("Me").
- Actions: "Copy all", click a chunk to copy or delete it, search, and a "Resume" control.
- https://docs.granola.ai/help-center/taking-notes/transcription.md

### Chat
- The chat bar sits at the bottom of the screen. What it searches depends on where you are: home searches all meetings, a note searches that meeting, a folder searches the folder. You can also tick several meetings and chat about them. https://docs.granola.ai/help-center/getting-more-from-your-notes/chatting-with-your-meetings.md
- Answers have had inline citations since Apr 2026. https://releasebot.io/updates/granola
- Slack posts contain the summary, action items, and a "Chat with this meeting" button. https://www.granola.ai/blog/two-dot-zero

### Onboarding / first run
- The steps are: sign in through the browser, choose calendars ("Select all"), then allow mic and system audio. https://docs.granola.ai/help-center/getting-started/setting-up-granola-for-the-first-time.md
- A one-time two-minute demo meeting from the co-founder is waiting on first sign-in. You land on a home that already shows your meetings and the demo note.
- A secondary source says there is no tour or carousel, and the populated list serves as onboarding.
- Jan 2026 polish: clearer permission-screen copy and friendlier sign-in errors. https://www.granola.ai/updates/whats-new-2026-01-08 , https://www.granola.ai/updates

### Empty states
- **Not verified.** Only the "Write notes..." placeholder in the note is visible. https://www.granola.ai/

### Typography
- The Feb 2 2026 rebrand pairs Quadrant, "a slightly mechanical slab serif for display", with Melange as "a neutral but subtly characterful UI font". https://www.granola.ai/blog/a-new-look-for-granola
- Melange is 14pt in the macOS app. https://www.abduzeedo.com/granola-brand-identity-design-ragged-edge
- Tokens extracted from the marketing site: Quadrant is always weight 400, at 20–68px, with about −1.5% tracking and line-height 1.0–1.17. Melange is used at 13–20px. https://www.shadcn.io/design/granola/raw
- In January the app font briefly moved from Inter to SF Pro. https://www.granola.ai/updates/whats-new-2026-01-08
- **Not verified:** where Quadrant appears inside the app, e.g. note titles. No source shows an app screen with it, and one secondary source describes the app as sans throughout.

### Motion people praise
- Little is documented.
- The verified points are the sidebar icon animation fix, focus-mode chrome fade, and performance-driven bar animation (above).

### What makes it feel crafted
- Stated goal: "Calm, but with energy underneath". https://www.granola.ai/blog/a-new-look-for-granola
- Small copy and behaviour fixes rather than big features: hiding share until a summary exists, "New note", friendlier errors, "10m left". https://www.granola.ai/updates/whats-new-2026-01-08 , https://www.granola.ai/blog/whats-new-2025-12-12

---

## B. Other apps (only ones I could verify)

### Notion AI Meeting Notes
https://www.notion.com/help/ai-meeting-notes
- The block has explicit "Start transcribing" and "Stop" actions, and the summary generates by itself after Stop.
- It needs at least 1 minute of audio (about 300 characters). We should say so in our "no audio" or failed state.
- "Retry summary" is offered.
- Hovering a citation shows the transcript snippet, and clicking jumps to that line.
- One action per calendar row, changing with time: "Join and transcribe" from 15 minutes before, "View summary" after. Same idea for our home rows: one state-driven action each.

### Linear
- Onboarding is 7 steps in about 1 minute, one decision per screen. Every optional step has a skip ("I'll do this later", "Skip for now"). It ends in the real product with a blinking cursor, with no tour or checklist. https://www.candu.ai/blog/linear-onboarding-teardown
- The 2026 refresh dimmed the sidebar, used fewer and smaller icons, removed coloured icon backgrounds, made separators fewer and softer, and moved to a warmer gray. Its principles: "Don't compete for attention you haven't earned" and "Structure should be felt not seen". https://linear.app/now/behind-the-latest-design-refresh

### Arc
- Onboarding is treated as an "unboxing": teach through play with small wins, ending in a personal membership card. https://www.inverse.com/input/design/the-browser-company-arc-design-interview
- Combined with Emil Kowalski's frequency rule: a screen seen once (our welcome) can carry flourish. Screens seen daily (home, review) should not. https://emilkowal.ski/ui/you-dont-need-animations

### Excluded
- I couldn't verify specific screens for Fathom, tl;dv, Read.ai, Krisp, Things 3 or Reflect from public sources, so I left them out.

---

## C. Motion recipes (installed versions: `motion` 13.4.6, React 19.3.0, checked in node_modules)

### General
- v11→v12 had no breaking changes. v13 only changed how `isValidProp` is injected, which matters for styled-components and Emotion. https://motion.dev/docs/react-upgrade-guide
- `staggerChildren` and `staggerDirection` are deprecated in 13.4.6 (checked in `node_modules/motion-dom/dist/index.d.ts`). Use `transition: { delayChildren: stagger(0.035) }` or `stagger(x, { from: "last" })`. https://motion.dev/docs/stagger
- `<MotionConfig reducedMotion="user">` turns off transform and layout animations but keeps opacity and colour. https://motion.dev/docs/react-accessibility
  - Inferred, not tested: this probably does not slow down spring motion values, so use `useReducedMotion()` and `jump()` for those.

### Shared title (list → document)
- Layout animations work by scaling, so text gets distorted. The docs recommend `layout="position"` for text and other elements whose aspect ratio changes. https://motion.dev/docs/react-layout-animations
- Our title also changes font (Pretendard 15/600 to serif 44), so a size morph will look stretched. Two options:
  - **(a)** Put `layoutId` with `layout="position"` on a wrapper `motion.div`, never on the `<input>`. The position glides and the size snaps. Or make the home row title serif too, so only the size changes. Verify that `layout="position"` combined with `layoutId` behaves as expected.
  - **(b)** Use `AnimateView name=…` from `motion/react-animate-view`, which needs React 19.3 (we have it). It crossfades snapshots, so the font change looks clean. But it can't be interrupted and needs `startTransition`. https://motion.dev/docs/react-animate-view It also needs Safari 18+ in WKWebView. https://developer.chrome.com/blog/view-transitions-in-2025
- If the home list scrolls inside an element rather than the window, add `layoutScroll` to that element. Otherwise the title flies in from the wrong place.
- With a shared `layoutId`, the transition used is the one on the element being animated **to** (the document title).

### `AnimatePresence mode="popLayout"` pitfalls
https://motion.dev/docs/react-animate-presence
- The direct child must put `ref` on a DOM node. In React 19, `PopChild` reads `children.props.ref` (checked in `framer-motion/dist/es/components/AnimatePresence/PopChild.mjs`). A screen component that doesn't pass `ref` through to its DOM node silently never pops.
- The parent must not be `position: static`.
- Exits can fail to run when interactions come quickly in popLayout. https://github.com/framer/motion/issues/2416 (still open)
- **CSP risk (inferred, needs testing):**
  - popLayout injects a `<style>` element into `document.head` at runtime.
  - Tauri adds nonces to the CSP at build time ("Tauri appends its nonces and hashes to the relevant CSP attributes automatically"). https://v2.tauri.app/security/csp/
  - Browsers ignore `'unsafe-inline'` once a nonce is present.
  - Test screen transitions in a `tauri build`, not only in `/preview.html`. If it's blocked, pass `nonce` through `MotionConfig` (`PopChild` reads it).

### Direction-aware screens
- Exiting components can't receive new props, so pass direction through `<AnimatePresence custom={dir}>` and variant functions, or read it with `usePresenceData()` (with `useIsPresent()`). https://motion.dev/docs/react-animate-presence
- Avoid `mode="wait"`: it roughly doubles total time. https://www.ui-skills.com/skills/raphaelsalaja/mastering-animate-presence

### Floating dock morph
- Wrapper: `<motion.div layoutRoot style={{ position: "fixed" }}>`. Fixed containers need `layoutRoot`.
- Pill: `<motion.div layout style={{ borderRadius: 28 }} transition={{ layout: spring.dock }}>`. The radius has to be in `style` for Motion to correct it during the morph. A Tailwind `rounded-full` class gets distorted. https://motion.dev/docs/react-layout-animations
- Inner content: `<AnimatePresence mode="popLayout" initial={false}>`, keyed by state, with children `layout="position"` so labels don't stretch.
- For the dock ↔ recording-screen waveform, share `layoutId` only on the background container and crossfade the bars. 12 bars vs. 2×24 bars can't morph one-to-one.

### Audio bars
- Write levels straight to `el.style.transform` (or MotionValues) inside the `recording://level` listener. Don't use React state or CSS transitions at 25Hz.
- Rounded bars warp under `scaleY`. Use square-ended bars, or Granola's two-cap translate trick. https://www.granola.ai/blog/dont-animate-height

### Number tickers
- Progress percentage: render the motion value directly. `motion.span` accepts `MotionValueNumber | MotionValueString` as children (checked in `framer-motion/dist/index.d.ts`), so there is no re-render per frame.
  ```tsx
  const s = useSpring(pct, { stiffness: 140, damping: 24 });
  const reduce = useReducedMotion();
  useEffect(() => { reduce ? s.jump(pct) : s.set(pct) }, [pct, reduce]);
  const text = useTransform(s, (v) => `${Math.round(v)}%`);
  return <motion.span className="tabular-nums">{text}</motion.span>;
  ```
- Timer: don't tween it. Roll each digit, 200ms with `ease.out`. Use Pretendard `tabular-nums`. The digit is a DOM `motion.span`, so popLayout works.
  ```tsx
  <span className="relative inline-block overflow-hidden">
    <AnimatePresence mode="popLayout" initial={false}>
      <motion.span key={d} className="inline-block"
        initial={{ y: "-60%", opacity: 0 }} animate={{ y: 0, opacity: 1 }} exit={{ y: "60%", opacity: 0 }} />
    </AnimatePresence>
  </span>
  ```
- `AnimateNumber` and `Typewriter` are Motion+ (paid). Skip both. https://motion.dev/docs/react-animate-number , https://motion.dev/docs/react-typewriter

### "AI writing" reveal
- Streamdown's approach (https://streamdown.ai/docs/animation):
  - Split into one `<span>` per word, use a CSS keyframe with `both` fill mode at 150ms, and only newly mounted spans animate.
  - For fast arrivals use `blurIn` at 200–300ms ease-out.
  - It doesn't handle reduced motion, so gate it with `motion-safe:` or `@media (prefers-reduced-motion: reduce) { animation: none }`.
- Our summary probably arrives whole. Reveal it by section with `stagger` (35–60ms, opacity plus 4px `y`) rather than per character.
- A `<textarea>` can't animate its contents. Reveal in a read-only layer (or the skeleton), then swap in the editor when the reveal ends.

### Timing
- Keep UI animations under 300ms. Don't animate actions started from the keyboard or things used constantly. Default to ease-out, make animations interruptible, and animate only `transform` and `opacity`.
- https://emilkowal.ski/ui/you-dont-need-animations , https://emilkowal.ski/ui/great-animations

---

## D. Korean typography rules

1. **Body line-height 1.6–1.75.**
   - Apple KR sets `body:lang(ko)` to 1.747 with 0em letter-spacing and `word-break: keep-all` (downloaded and checked). https://www.apple.com/kr/app-store/styles/main.built.css
   - A Korean practitioner guide says about ×1.6. https://yozm.wishket.com/magazine/detail/1017/
   - Our `body-doc` at 1.75 is right.
2. **Headings: line-height about 1.15–1.2, tracking near 0.**
   - Apple KR headlines use 1.155–1.19 and **0em**. https://www.apple.com/kr/app-store/styles/main.built.css
   - The guide says to tighten tracking as size grows, and about −0.3 for 16–17pt body. https://yozm.wishket.com/magazine/detail/1017/
   - Granola's Latin settings (line-height 1.0, −1.5%) are too tight for Hangul that wraps onto two lines. Keep serif Hangul at −0.01em to −0.02em at most.
3. **Weight: match Instrument Serif's single 400.**
   - Granola's serif display is always 400. https://www.shadcn.io/design/granola/raw
   - Set Noto Serif KR display to about 400–500 instead of 600, and check mixed titles like "Q3 로드맵 리뷰".
   - In typical text fonts, Latin and numerals are drawn smaller than Hangul, so check optical size too. https://toss.im/tossfeed/article/beginning-of-tps
4. **Line breaking: `word-break: keep-all` plus `overflow-wrap: anywhere`.**
   - `keep-all` stops breaks inside CJK words. https://developer.mozilla.org/en-US/docs/Web/CSS/Reference/Properties/word-break
   - `anywhere` protects long URLs and channel names.
   - `keep-all` can't break long compound words that have no spaces, and Korean print traditionally breaks body text between syllables. https://lists.w3.org/Archives/Public/public-css-archive/2019Sep/0569.html
   - It's the right default for short UI copy and titles; long Korean prose can wrap less evenly.
   - Set `<html lang="ko">`.
5. **`text-wrap`**
   - `balance` for headings needs Safari 17.5+. `pretty` needs Safari 26+. https://blog.logrocket.com/css-text-wrap-balance-vs-text-wrap-pretty/ , https://mjtsai.com/blog/2025/04/11/css-text-wrap-pretty-in-webkit
   - On macOS, Tauri uses the system WebKit, so support depends on the user's OS. Treat both as extras that may not apply.
6. **Numbers:** fixed-width digits for values that change live, proportional digits for single large static figures. https://toss.im/tossfeed/article/beginning-of-tps This matches the current design-system.md.
7. **Pretendard:** use the "Pretendard Variable" dynamic subset. WebKit can render weights wrongly unless a weight range is declared (the README's example is `'45 920'`). https://github.com/orioncactus/pretendard
8. **Context**
   - In Korean print, the serif (Myeongjo/Batang) was the body face and the sans (Dotum) the display face. https://ccjktype.fonts.adobe.com/2013/05/gothic-myungjo-or-dotum-batang.html
   - Serif headings over sans UI flips that, which reads as editorial.
   - **Not found:** a verified Korean product example of serif display mixed with sans UI.

---

## E. Recommended direction: (B) date timeline + floating dark dock

### Why B
- Granola keeps the meeting list in the main pane, grouped by time. Its sidebar only holds Spaces and folders, and we have neither, so (A) would repeat home in a sidebar and squeeze the 680px document.
- Granola's live state is a small element at the bottom of one light canvas, not a separate dark mode. (C) would add a second theme while we ship light only.

### Structure
- **Home:** one column, rows not cards.
  - Add a top group for "needs you" (in progress, waiting for review), Granola's "Coming up" adapted to us.
  - Below it, date groups (오늘, 어제 …).
  - One state-driven action per row, as in Notion.
- **Dock:** bottom center, dark pill, the only live element.
  - When the app isn't focused, use the tray instead of Granola's floating nub.
  - A nub would need an extra always-on-top window; add it only if users lose track of recording.
- **Processing becomes the document in a "writing" state:** same title position and meta line, skeleton turning into text in place. Granola generates inside the note, so this feels like continuity rather than navigating to another screen.
- **Review:**
  - The send bar appears only once a summary exists, as Granola does with share.
  - Transcript is a right-hand panel with 나/상대 bubbles, if our pipeline keeps which track (mic or system) each segment came from. **Unverified:** I didn't check this.
  - Optional focus fade: the header and send bar dim while the user types.
  - Bullet-to-transcript citations: later, because they need timestamps from the LLM.
- **Welcome:** Linear-style, one decision per step with skip links, ending on home with the record button focused. Allowed one flourish, since it's seen once (Arc).
- **Settings:** left tabs as planned. Nothing found that argues otherwise.

### Changes to `docs/design-system.md`
1. Noto Serif KR display weight from 600 to about 400–500, to match Instrument Serif 400.
2. Dock and recording bars: square ends or two caps, not rounded bars under `scaleY`. Update levels outside React state.
3. Shared title: `layoutId` on a wrapper with `layout="position"` (or `AnimateView`), not on the input. Add `layoutScroll` on the home scroller.
4. Dock radius in `style`, not a class.
5. Test popLayout under the built Tauri CSP.
6. Replace `staggerChildren` (deprecated) with `delayChildren: stagger()` wherever it is used.

### Don't do
- Purple/blue gradients, abstract swirls, a geometric sans as the identity, cool RGB blues, or "sleek, futuristic" visuals. https://www.hrzn.co.uk/insights/what-the-granola-rebrand-tells-you-about-branding-an-ai-product , https://gesso.substack.com/p/granolas-rebrand-doesnt-make-it-look , https://designcompass.org/en/2026/07/14/granola-rebrand-case-study-ai-notepad/
  - Our #5550FF sits close to this cliché, so use it only for small accents, never in gradients or glows.
- Animating height or layout properties, or CSS transitions on high-frequency data. https://www.granola.ai/blog/dont-animate-height
- UI motion over 300ms, animated keyboard actions, or delight on things seen daily. https://emilkowal.ski/ui/you-dont-need-animations
- Tours, carousels, checklists, or setup gates. https://www.candu.ai/blog/linear-onboarding-teardown
- Lots of icons, coloured icon backgrounds, or heavy separators. https://linear.app/now/behind-the-latest-design-refresh
- A character-by-character typewriter on a whole summary, or a modal "generating" state.
- `AnimatePresence mode="wait"` for screens, because it doubles the time. https://www.ui-skills.com/skills/raphaelsalaja/mastering-animate-presence

### Unverified, to check
- Granola's in-app serif use, its enhance and generating animations, and its empty states.
- How `layout="position"` behaves together with `layoutId`.
- The popLayout CSP behaviour in a built Tauri app.
- Whether reduced motion slows down `useSpring`.
- Whether our transcripts keep mic/system track labels.
- Screen-level UI of Fathom, tl;dv, Read.ai, Krisp and Things 3.
