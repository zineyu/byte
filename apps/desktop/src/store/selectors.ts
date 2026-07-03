import type { ChatMessage, RuntimeEventLogEntry, TimelineItem } from "./types";
import { getMessageBodyToolCalls } from "./types";

export type EventGroup = {
  event: RuntimeEventLogEntry;
  count: number;
};

// Number of events shown in the timeline UI. Must be <= MAX_EVENTS in
// reducer.ts, which caps the in-memory event log.
export const TIMELINE_MAX_EVENTS = 32;

export function groupEvents(events: RuntimeEventLogEntry[]): EventGroup[] {
  const result: EventGroup[] = [];
  const limit = Math.min(events.length, TIMELINE_MAX_EVENTS);
  for (let index = 0; index < limit; index += 1) {
    const event = events[index];
    if (event.type === "state_changed" && result.length > 0) {
      const last = result[result.length - 1];
      if (
        last.event.type === "state_changed" &&
        last.event.state.status === event.state.status
      ) {
        last.count += 1;
        continue;
      }
    }
    result.push({ event, count: 1 });
  }
  return result;
}

export function buildTimelineItems(messages: ChatMessage[]): TimelineItem[] {
  const items: TimelineItem[] = [];
  for (const message of messages) {
    if (message.role === "summary") {
      items.push({ type: "summary", id: message.id, message });
      continue;
    }
    // Tool result messages are visualized through their corresponding tool
    // call cards, not as standalone timeline entries.
    if (message.role === "tool") {
      continue;
    }
    // Assistant messages that contain only tool calls (no text) should be
    // represented entirely by their tool_call cards, not as an empty bubble.
    const hasText = message.content.trim().length > 0;
    if (message.role === "assistant" && !hasText) {
      for (const call of getMessageBodyToolCalls(message.body)) {
        items.push({
          type: "tool_call",
          id: `tool-${call.id}`,
          toolCallId: call.id,
        });
      }
      continue;
    }
    items.push({ type: "message", id: message.id, message });
    if (message.role === "assistant") {
      for (const call of getMessageBodyToolCalls(message.body)) {
        items.push({
          type: "tool_call",
          id: `tool-${call.id}`,
          toolCallId: call.id,
        });
      }
    }
  }
  return items;
}
