---
name: Byte Agent
description: A luminous, minimal interface for a local coding assistant. The design language is soft, rounded, and white-first, with a calm blue accent for modes and status, and a dark neutral for authority and text.
version: alpha
colors:
  background: "#ffffff"
  background-subtle: "#f9fafb"
  background-hover: "#f2f3f5"
  primary: "#1a1a1a"
  primary-hover: "#333333"
  on-primary: "#ffffff"
  text-primary: "#111827"
  text-body: "#1f2937"
  text-secondary: "#6b7280"
  text-muted: "#9ca3af"
  accent: "#3b82f6"
  accent-subtle: "#eff6ff"
  accent-text: "#1e3a8a"
  success: "#22c55e"
  success-soft: "#d1fae5"
  success-text: "#14532d"
  error: "#ef4444"
  error-soft: "#fee2e2"
  error-subtle: "#fef2f2"
  error-text: "#991b1b"
  warning-subtle: "#fffbeb"
  warning-text: "#78350f"
typography:
  display:
    fontFamily: "Inter, ui-sans-serif, system-ui, sans-serif"
    fontSize: "2.5rem"
    fontWeight: 500
    letterSpacing: "-0.02em"
  headline:
    fontFamily: "Inter, ui-sans-serif, system-ui, sans-serif"
    fontSize: "1.25rem"
    fontWeight: 700
  body:
    fontFamily: "Inter, ui-sans-serif, system-ui, sans-serif"
    fontSize: "1rem"
    lineHeight: 1.6
    fontWeight: 400
  body-chat:
    fontFamily: "Inter, ui-sans-serif, system-ui, sans-serif"
    fontSize: "1rem"
    lineHeight: 1.65
  label:
    fontFamily: "Inter, ui-sans-serif, system-ui, sans-serif"
    fontSize: "0.9rem"
    fontWeight: 500
  caption:
    fontFamily: "Inter, ui-sans-serif, system-ui, sans-serif"
    fontSize: "0.875rem"
    fontWeight: 400
  small:
    fontFamily: "Inter, ui-sans-serif, system-ui, sans-serif"
    fontSize: "0.8125rem"
    lineHeight: 1.5
  overline:
    fontFamily: "Inter, ui-sans-serif, system-ui, sans-serif"
    fontSize: "0.75rem"
    fontWeight: 600
    letterSpacing: "0.025em"
    textTransform: uppercase
  code:
    fontFamily: "ui-monospace, SFMono-Regular, Menlo, Monaco, Consolas, monospace"
    fontSize: "0.8125rem"
    lineHeight: 1.5
rounded:
  pill: "999px"
  2xl: "28px"
  xl: "22px"
  lg: "18px"
  md: "14px"
  sm: "10px"
  xs: "8px"
spacing:
  0: "0"
  xs: "0.25rem"
  sm: "0.5rem"
  md: "0.75rem"
  lg: "1rem"
  xl: "1.25rem"
  2xl: "1.5rem"
  3xl: "2rem"
  4xl: "3rem"
  2-5: "0.65rem"
  4-5: "1.1rem"
components:
  input-card:
    backgroundColor: "{colors.background}"
    textColor: "{colors.text-body}"
    rounded: "{rounded.2xl}"
    padding: "1rem 1.25rem"
  input-card-focus:
    backgroundColor: "{colors.background}"
  send-button:
    backgroundColor: "{colors.primary}"
    textColor: "{colors.on-primary}"
    rounded: "{rounded.pill}"
    size: "2.25rem"
  send-button-hover:
    backgroundColor: "{colors.primary-hover}"
  nav-item:
    backgroundColor: "{colors.background}"
    textColor: "#4b5563"
    rounded: "{rounded.sm}"
    padding: "0.6rem 0.75rem"
  nav-item-active:
    backgroundColor: "{colors.background-hover}"
    textColor: "{colors.text-primary}"
  session-row-hover:
    backgroundColor: "{colors.background-hover}"
    textColor: "{colors.text-body}"
  hero-title:
    textColor: "{colors.text-primary}"
    typography: "{typography.display}"
  hero-subtitle:
    textColor: "{colors.text-secondary}"
    typography: "{typography.body}"
  placeholder:
    textColor: "{colors.text-muted}"
    typography: "{typography.body}"
  message-developer:
    backgroundColor: "{colors.background-subtle}"
    textColor: "{colors.text-body}"
    rounded: "{rounded.lg}"
  message-assistant:
    backgroundColor: "{colors.background}"
    textColor: "{colors.text-body}"
    rounded: "0"
  message-summary:
    backgroundColor: "{colors.warning-subtle}"
    textColor: "{colors.warning-text}"
    rounded: "{rounded.md}"
  message-timestamp:
    textColor: "{colors.text-secondary}"
    typography: "{typography.small}"
  tool-call-card:
    backgroundColor: "{colors.background-subtle}"
    textColor: "{colors.text-body}"
    rounded: "{rounded.md}"
  tool-call-card-running:
    backgroundColor: "{colors.accent-subtle}"
    textColor: "{colors.accent}"
    rounded: "{rounded.md}"
  tool-call-card-error:
    backgroundColor: "{colors.error-subtle}"
    textColor: "{colors.error}"
    rounded: "{rounded.md}"
  tool-status-badge:
    backgroundColor: "{colors.background-subtle}"
    textColor: "{colors.text-body}"
    rounded: "{rounded.pill}"
  tool-status-badge-completed:
    backgroundColor: "{colors.success-soft}"
    textColor: "{colors.success-text}"
  tool-status-badge-running:
    backgroundColor: "{colors.accent-subtle}"
    textColor: "{colors.accent-text}"
  tool-status-badge-error:
    backgroundColor: "{colors.error-subtle}"
    textColor: "{colors.error-text}"
  tool-call-diff:
    backgroundColor: "{colors.background}"
    textColor: "{colors.text-body}"
    rounded: "12px"
    typography: "{typography.code}"
  tool-call-diff-line-summary:
    backgroundColor: "{colors.background-subtle}"
    textColor: "{colors.text-secondary}"
  tool-call-diff-line-header:
    textColor: "{colors.text-secondary}"
  tool-call-diff-line-hunk:
    backgroundColor: "{colors.background-hover}"
    textColor: "{colors.text-body}"
  tool-call-diff-line-delete:
    backgroundColor: "{colors.error-soft}"
    textColor: "{colors.error-text}"
  tool-call-diff-line-insert:
    backgroundColor: "{colors.success-soft}"
    textColor: "{colors.success-text}"
  tool-call-diff-line-context:
    textColor: "{colors.text-body}"
  tool-call-command-line:
    backgroundColor: "{colors.background-subtle}"
    textColor: "{colors.text-primary}"
    rounded: "{rounded.xs}"
    typography: "{typography.code}"
  tool-call-command-prompt:
    textColor: "{colors.text-secondary}"
  tool-call-command-text:
    textColor: "{colors.text-primary}"
  tool-call-exit-code-badge:
    backgroundColor: "{colors.background-subtle}"
    textColor: "{colors.text-body}"
    rounded: "{rounded.pill}"
    typography: "{typography.code}"
  tool-call-exit-code-badge-success:
    backgroundColor: "{colors.success-soft}"
    textColor: "{colors.success-text}"
  tool-call-exit-code-badge-error:
    backgroundColor: "{colors.error-soft}"
    textColor: "{colors.error-text}"
  markdown-body:
    textColor: "{colors.text-body}"
    typography: "{typography.body-chat}"
  markdown-heading:
    textColor: "{colors.text-primary}"
    typography: "{typography.headline}"
  markdown-link:
    textColor: "{colors.accent}"
  markdown-inline-code:
    backgroundColor: "{colors.background-hover}"
    textColor: "{colors.text-body}"
    typography: "{typography.code}"
    rounded: "6px"
  markdown-blockquote:
    backgroundColor: "{colors.accent-subtle}"
    textColor: "{colors.text-body}"
    typography: "{typography.body}"
  markdown-image:
    backgroundColor: "{colors.background}"
    textColor: "{colors.text-body}"
    rounded: "{rounded.md}"
  markdown-code-block:
    backgroundColor: "{colors.background}"
    textColor: "{colors.text-body}"
    rounded: "{rounded.md}"
  markdown-code-block-header:
    backgroundColor: "{colors.background-subtle}"
    textColor: "{colors.text-secondary}"
    typography: "{typography.small}"
  markdown-code-block-body:
    backgroundColor: "{colors.background}"
    textColor: "{colors.text-body}"
    typography: "{typography.code}"
  status-badge:
    backgroundColor: "{colors.background-subtle}"
    textColor: "{colors.text-body}"
    rounded: "{rounded.pill}"
  drawer:
    backgroundColor: "{colors.background}"
  sidebar:
    backgroundColor: "{colors.background-subtle}"
  brand-header:
    textColor: "{colors.text-primary}"
    typography: "{typography.headline}"
  mode-tab:
    backgroundColor: "transparent"
    textColor: "{colors.text-secondary}"
    rounded: "{rounded.pill}"
  mode-tab-active:
    backgroundColor: "{colors.background}"
    textColor: "{colors.text-primary}"
  mode-tab-hover:
    textColor: "{colors.text-body}"
  error-banner:
    backgroundColor: "{colors.error-subtle}"
    textColor: "{colors.error-text}"
    rounded: "{rounded.sm}"
  mode-badge:
    backgroundColor: "{colors.accent-subtle}"
    textColor: "{colors.accent}"
    rounded: "{rounded.pill}"
  beta-badge:
    backgroundColor: "{colors.accent-subtle}"
    textColor: "{colors.accent}"
    rounded: "{rounded.pill}"
  delete-button-hover:
    backgroundColor: "{colors.error-soft}"
    textColor: "{colors.error}"
    rounded: "{rounded.xs}"
  connection-online:
    textColor: "{colors.success}"
    rounded: "{rounded.pill}"
  connection-offline:
    textColor: "{colors.error}"
    rounded: "{rounded.pill}"
---

## Overview

Byte Agent is a local desktop coding assistant. Its interface should feel airy, approachable, and quietly competent — more like a clean workspace than a traditional chat client. The design language is white-first, softly rounded, and spacious, with a calm blue accent that marks modes, previews, and the developer's presence. Authority comes from the dark neutral type and crisp hierarchy, not from heavy surfaces or saturated color.

## Colors

The palette is almost monochromatic, letting the content breathe. Color is used only to mark mode, status, or sender identity.

- **Primary (#1a1a1a):** Near-black for the primary action button, brand authority, and key text. A softer dark than pure black.
- **Primary Hover (#333333):** Slightly lighter neutral for hovered primary surfaces.
- **Background (#ffffff):** Clean white for the main canvas, cards, and drawers.
- **Background Subtle (#f9fafb):** Soft gray for user message bubbles, tool cards, status badges, and hovered list rows.
- **Background Hover (#f2f3f5):** Hover states for navigation items, session rows, and neutral buttons.
- **Text Primary (#111827):** Strong headings, hero title, and active navigation.
- **Text Body (#1f2937):** Default body and message text.
- **Text Secondary (#6b7280):** Captions, metadata, placeholder text, and muted labels.
- **Text Muted (#9ca3af):** Disabled controls and subtle hints.
- **Accent (#3b82f6):** Agent mode badges, Beta Preview labels, and active tool-call states. The single source of interactive color.
- **Accent Text (#1e3a8a):** Darker blue variant for accessible text on light blue surfaces, such as tool avatars and running status badges.
- **Success (#22c55e):** Online connection indicator and successful tool-call status.
- **Success Text (#14532d):** Darker green variant for accessible text on light green surfaces, such as completed status badges.
- **Error (#ef4444):** Offline indicator, deletion hover, and error banners.
- **Warning Text (#78350f) / Warning Subtle (#fffbeb):** Summary/compaction entries, paired with a warm cream background to separate them from normal chat turns.

Borders are rendered with light grays: `#e8eaed` for most cards and inputs, `#f1f3f4` for subtle separators inside the sidebar, and `#d1d5db` for focused inputs.

## Typography

All UI text uses Inter or the system sans-serif stack for clarity. Code snippets and tool arguments use a system monospace stack.

- **Display (2.5rem, weight 500):** Hero headline on the empty state. Generous size with slightly tighter letter spacing for a polished, friendly feel.
- **Headline (1.25rem, weight 700):** Brand title and drawer section titles.
- **Body (1rem, line-height 1.6):** Default prose, hero subtitle, and input placeholders.
- **Body Chat (1rem, line-height 1.65):** Chat message content; slightly more open for readability.
- **Label (0.9rem, weight 500):** Navigation items, session rows, and tool names.
- **Caption (0.875rem):** Drawer body text, connection status, and tool output prose.
- **Small (0.8125rem, line-height 1.5):** Tool call headers, file listings, and compact content.
- **Overline (0.75rem, weight 600, uppercase):** Section headers such as "Workspace Instructions" and summary labels.
- **Code (0.8125rem, monospace):** Inline code, tool arguments, and grep results.

## Layout

The application uses a three-column grid with generous whitespace:

1. **Left sidebar** (240px): brand header with the Sparkles icon and "Byte" title, top mode tabs (`Chat` / `Work`), a scrollable tab panel (session list under `Chat`, workspace info under `Work`), grouped navigation (`运行时`, `设置`), and footer connection status. The sidebar uses the subtle background (`#f9fafb`) to recede from the main white canvas, relying on spacing and subtle hover states to create hierarchy.
2. **Main area** (flexible): centered empty-state hero with a large rounded input card, or a chat conversation stream when messages exist.
3. **Right drawer** (360px, collapsible): runtime events and settings, fixed to viewport height and scrolling independently.

On medium screens (≤900px) the sidebar narrows to 200px and the right drawer becomes a fixed overlay. On small screens (≤680px) the sidebar collapses to a top bar, hiding session lists, mode tabs, and footer, and the right drawer becomes full-width.

The layout favors centered, contained content over full-bleed surfaces. The empty state anchors the input card in the center of the viewport with plenty of surrounding space.

## Elevation & Depth

Elevation is extremely subtle, used only to lift the input card and drawer above the flat canvas:

- **Input card:** `0 2px 16px rgba(0,0,0,0.03)` at rest, intensifying to `0 8px 32px rgba(0,0,0,0.06)` on focus. The border is a whisper rather than a hard line.
- **Right drawer overlay:** `-4px 0 24px rgba(0,0,0,0.05)` when it floats above the main area on narrow screens.
- **Active tool cards:** a faint colored border (`#93c5fd` for running, `#fca5a5` for error) instead of a shadow lift.

No heavy shadows, gradients, or material layers. Depth is created through typography scale, spacing, and the gentle shadow of the input card.

## Shapes

- **Pill (999px):** Status badges, mode badges, Beta Preview badge, send button, and tool status badges.
- **2x Large (28px):** Main input card border radius. The largest, most prominent radius in the interface.
- **Large (18px):** Chat message bubbles and workspace-instruction cards.
- **Medium (14px):** Summary cards and tool call cards.
- **Small (10px):** Navigation items and session rows.
- **Extra Small (8px):** Drawer close buttons and deletion buttons.
- **2x Small (6px):** Inline code blocks.

Radius is used to make the interface feel friendly and approachable. Cards are significantly rounder than sharp utility panels.

## Components

### Input card

A large, softly rounded surface for composing messages. It uses a white background, 28px radius, a very light border (`#e8eaed`), and a gentle shadow. The footer is separated by a light border and holds tool actions on the left and the mode badge + circular send button on the right. The textarea is transparent, borderless, and auto-growing up to roughly eight lines; beyond that it scrolls internally. On focus the border shifts to `#d1d5db` and the shadow deepens.

### Brand header

The brand header sits at the top of the left sidebar and contains the Sparkles icon and the "Byte" wordmark. The icon uses the accent blue to mark the agent identity; the wordmark uses the headline typography and primary text color. It is compact and unobtrusive, acting as a persistent anchor without visually competing with the content below.

### Mode tabs

At the top of the sidebar, a compact segmented control switches between the `Chat` and `Work` sidebar views. The active tab uses a white background with dark text; inactive tabs use muted text on a transparent background. The container is pill-shaped with a subtle hover gray background. Focus rings use the accent blue outline.

### Navigation

Sidebar navigation items are full-width buttons with a 10px radius. Default state uses transparent background with secondary text; hover uses the hover gray; active uses the hover gray with primary text. Primary actions (`新对话` in Chat, `打开工作区` in Work) are outlined with light borders to sit cleanly against the sidebar. The `运行时` and `设置` items live in a grouped navigation block at the bottom of the sidebar above the footer. Section labels (e.g., Project, Tasks) use small, muted uppercase text to group items without heavy separators.

### Hero empty state

A centered, spacious composition: a brand icon above a large display headline, a small Beta Preview badge beneath the headline, and the input card below. The icon is friendly and colorful, but the rest of the state remains neutral.

The chat stream itself is a full-width scroll container so its scrollbar sits at the far right of the main area, while individual messages stay centered within an 800px reading column.

### Chat messages

Messages are arranged without avatars to keep the stream clean. Developer messages are rendered as right-aligned rounded bubbles using the subtle gray background (`#f9fafb`) with an 18px radius, capped at 75% of the chat width so they feel like outgoing chat bubbles. Assistant messages are rendered as plain left-aligned text with no bubble background or padding, letting the content read like a document on the white chat canvas. Each completed message shows a small timestamp below its content in the secondary text color, right-aligned for the developer and left-aligned for the assistant. Summary/compact entries are full-width cards with warm warning tones, a `#fde68a` border, and a header row that separates the "会话摘要" label from its timestamp.

### Tool call cards

Rendered below the assistant message body, tool call cards are centered in the chat area with a max-width of 80%. They use a soft gray card surface (`#f9fafb`) with a subtle `#f1f3f4` border and an 18px radius. By default the body is collapsed so only the header row is visible: a plain tool-type icon, the monospace tool-call signature, a pill-shaped status badge on the right (`运行中` / `已完成` / `失败`), and a chevron toggle. Clicking the toggle expands the card to reveal the tool output, and the card may be expanded while a tool is running to show live streaming output. Running states shift to a light blue background with a `#bfdbfe` border; error states shift to a light red background with a `#fecaca` border. Tool output is rendered inside a clean white nested surface with a light `#e8eaed` border and a 12px radius — directory listings show a path caption and item count above the list, file contents show a caption header above a pre block, grep results show monospace match lines, `apply_patch` / `write_file` results show a unified diff with green insertions, red deletions, and gray context/hunk lines, and `run_command` results show the command line on a subtle gray line with a light border, followed by the combined stdout/stderr output and a pill-shaped exit-code badge (green for `0`, red for non-zero). Directory and file list items carry a muted icon and a generous row padding to stay scannable. Long lists scroll internally.

### Status badges and connection

Connection status uses a small dot (green for online, red for offline) beside a label. Status badges in the drawer are pill-shaped with a light border and bold value text. Mode badges (Agent, Ask, Beta Preview) use the accent subtle background with the accent text color.

### Runtime events panel

The right drawer contains a collapsible **运行时事件** panel alongside **设置**. Events are timestamped, tagged by type, and displayed as compact cards using the subtle background (`#f9fafb`) and light border (`#e8eaed`). Consecutive `state_changed` events with the same daemon status are collapsed into a single card with a counter (e.g., `×3`). Error events use the error-subtle background and error-text color. Long tool output is rendered in a monospace pre block with a max-height and overflow. The panel scrolls independently within the drawer.

### Rendered Markdown body

Developer and assistant messages are rendered as Markdown once a message has finished streaming. The rendered body uses the `.markdown-body` scope, which overrides the default browser Markdown styles to match the design system:

- Paragraphs, lists, and headings inherit the chat body typography (`body-chat`) and use the `text-body` color; headings use `text-primary` and the headline weight.
- Links use the accent blue and are opened in a new tab with safe `rel` attributes.
- Inline code uses the code typography and a subtle gray background (`background-hover`) with a 6px radius.
- Fenced code blocks are wrapped in a card with a header showing the language label and a copy button. The header uses the subtle background (`background-subtle`) and secondary text; the body uses the code typography and a white background. Syntax highlighting uses a minimal Prism token palette derived from the existing color tokens.
- Blockquotes use a soft accent left border and the accent-subtle background.
- Tables (GitHub-Flavored Markdown) render with a full-width layout, light borders (`#e8eaed`), a subtle header background (`#f9fafb`), and alternating row backgrounds. Wide tables scroll horizontally within the message bubble if needed.
- Task lists and strikethrough text use the standard GFM rendering, with checkboxes shown as disabled and strikethrough text using the muted text color.
- Images are allowed and rendered at max-width 100% with the medium radius so they stay within the message bubble.

During streaming, the message remains plain text with `white-space: pre-wrap` and a blinking cursor; Markdown rendering is only applied after the message is completed.

## Do's and Don'ts

- Do keep the interface white-first and spacious. Generous padding and centered content are central to the feel.
- Do use the large 28px radius for the main input card and generous 18px radius for messages.
- Do use the accent blue only for modes, previews, status, and the developer's identity. Avoid using it for generic decoration.
- Do collapse repeated runtime events and truncate long output to keep the right drawer scannable.
- Don't introduce additional saturated colors beyond the blue accent and green/red status palette.
- Don't use heavy shadows, gradients, or dark backgrounds. The interface should feel light and airy.
- Don't make the input card visually compete with the chat content; it should recede until focused.
