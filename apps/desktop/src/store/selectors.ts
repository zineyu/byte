import type { ChatMessage, RuntimeEventLogEntry, TimelineItem } from "./types";

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
    items.push({ type: "message", id: message.id, message });
    if (message.role === "assistant" && message.toolCalls) {
      for (const call of message.toolCalls) {
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
