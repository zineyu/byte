# Desktop UI Guidelines

Conventions for the Byte Agent desktop interface (`apps/desktop`).

## Layout

- **Left sidebar**: top tabs (`Chat` / `Work`), main navigation (`新对话`, `运行时`, `设置`), and footer connection status.
- **Main area**: empty-state hero with centered input card; switches to a conversation stream once messages exist.
- **Right drawer**: collapsible panels for `运行时事件` and `设置`. Fixed to viewport height and scrolls independently.

## Icons

Use `lucide-react` linear icons throughout the UI. Current icons:

- Brand: `Sparkles`
- New chat: `Plus`
- Runtime: `Zap`
- Settings: `Settings`
- Attach / tool: `Plus`
- Ask mode: `MessageSquare`
- Send: `ArrowUp`
- Close drawer: `X`
- User avatar: `User`
- Assistant avatar: `Bot`

## Input card

- Large rounded card (`border-radius: 22px`) with a subtle shadow.
- Textarea uses transparent background, no border, auto-growing rows.
- Footer split into left tools and right actions (`Chat` mode badge + circular send button).
- `Enter` sends; `Shift+Enter` inserts a newline.

## Colors

- Background: white (`#ffffff`)
- Sidebar: light gray (`#f9fafb`)
- Primary surface: dark slate (`#172033`)
- Developer message bubble: blue tint (`#eff6ff`)
- Assistant message bubble: gray tint (`#f9fafb`)
- Connection online: green; offline: red

## Runtime events

- Events are timestamped, tagged by type, and detail-truncated.
- Consecutive `state_changed` events with the same status are collapsed with a `×N` counter.
- The event list lives in the right drawer and scrolls independently.
