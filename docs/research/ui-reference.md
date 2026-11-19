# slackless UI reference (match the official Slack desktop client)

Target screenshot (user-provided, 2026-11-19): `screenshots/slack-reference-dm.png`
(official Slack desktop, Norce workspace, **dark** theme, a DM open). slackless must be a
carbon copy of this. Build the Core UI and the visual-polish pass against it.

## Overall layout — four vertical regions, dark theme

1. **Title / global bar** (top, full width): macOS traffic lights, back/forward/history
   arrows, a wide centered search pill "Describe what you are looking for", a workspace/user
   switcher, help (?). Dark, ~44px tall. (GPUI Kit `TitleBar`.)
2. **Workspace rail** (far left, ~70px): rounded workspace icon(s) stacked at top with the
   active one highlighted + its short name, a "+" to add; then large nav icons with labels
   stacked vertically: Home (house, active = white), DMs (badge count), Activity, Files,
   To-dos, Agents & tools, Admin; at the very bottom a "+" , a theme toggle (moon), and the
   user avatar. Darkest surface.
3. **Channel sidebar** (~300px): workspace header row "Norce ⌄" with gear + compose-message
   icons; a "Find a conversation…" input; then a scrollable list:
   - channels as `# name`, **bold white when unread**, muted gray when read; a floating
     "↑ More unreads" pill; collapsible **section headers** in caps (e.g. "STATUS") and the
     built-in "Channels" / "Direct messages" groups (each with a +, and a ⋮ "More actions"
     on hover).
   - Direct messages list each with a small round **avatar + presence dot**, display name;
     group DMs show a stacked/numbered avatar.
   - The **selected** row has a solid blue background (#1164A3-ish); hovered rows a subtle
     lighter bg.
4. **Message pane** (fills the rest):
   - **Conversation header**: ☆ star, the peer avatar + name (or `#` channel), then on the
     right huddle (headphones ⌄), notifications bell, search, ⋮ more. Below it a tab row:
     **Messages** (active, underlined) · Add canvas · Files & links · +.
   - **Message list**: avatar + **bold name** + optional role/badge ("6 years 🎉") + timestamp
     on the first message of a group; **consecutive messages from the same author omit the
     avatar/name** and show only (on hover) a small timestamp. A centered **date-divider pill**
     ("Today ⌄"). An inline **link unfurl**. **Reaction chips** under a message (`👍 1` + an
     add-reaction button). A full-width **"New" divider** (orange line, right-aligned "New"
     label) marking first unread.
   - **Composer** (bottom, boxed): a formatting toolbar row (Bold, Italic, Strikethrough,
     format ¶, link, ordered/unordered/checklist lists, code, code-block, blockquote);
     a "Message <name>" placeholder; a bottom action row (+, Aa format toggle, emoji, @mention,
     video clip, audio clip, slash) and a send button on the right.

## Theming — multiple themes, not just dark

The reference screenshot is **one** theme (Dark). slackless must reproduce Slack's full
theming system, switchable in **Preferences → Appearance**:

- **Light** and **Dark** (and **System**/sync-with-OS), via gpui-kit `Theme::change` +
  `ThemeMode` (Light/Dark/System — see gpui-kit-guide section 11).
- Slack's built-in **custom themes** (Aubergine, and the other named colour sets) and a
  **user-customisable theme** (the sidebar/accent colour slots Slack exposes).
  The LAYOUT below is theme-independent; only the palette swaps. Drive every colour through
  semantic theme tokens (`cx.theme()`), never hard-coded hex, so all themes work from one set
  of views. Model the palette as a Slack-style theme (background, foreground, muted, accent,
  selected, hover, link, mention, presence) that each named theme fills in.

## Palette for the Dark theme instance (approximate — one theme among several)

- Sidebar / rail bg: ~#1A1D21 (rail slightly darker). Message pane bg: ~#1A1D21.
- Primary text #D1D2D3; secondary/muted #ABABAD; channel-read #9A9B9D.
- Selected row bg #1164A3, text white; hover bg rgba(255,255,255,0.06).
- Links #1D9BD1. "New" divider + mention red #E01E5A. Green presence dot #2BAC76.
- Section headers: small, upper, muted, with a disclosure triangle.

## Behaviors to reproduce

Message grouping by author+time window; hover actions (react / reply in thread / ⋮);
unread bold + badge counts; presence dots; date dividers; the "New" unread divider;
bottom-anchored list that follows new messages (MessageScroller tail-follow).

## How this is used

- Phase 4 verify compares slackless's fixture-mode screenshot against this reference.
- A visual-polish pass (feature-parity wave) tunes spacing, colors, and the Slack-Dark theme
  until slackless is visually indistinguishable from this screenshot at the same width.
