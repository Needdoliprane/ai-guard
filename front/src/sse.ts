import { isAuditEvent } from "./model.ts";
import type { AuditEvent } from "./types.ts";

export interface SseMessage {
  event: string;
  data: string;
  id: string;
  retry: number | undefined;
}

export function createSseParser(
  onMessage: (message: SseMessage) => void,
  maxBytes = 2 * 1024 * 1024,
) {
  let buffer = "";
  let data: string[] = [];
  let event = "";
  let id = "";
  let retry: number | undefined;
  let size = 0;
  let first = true;
  function line(value: string) {
    if (value === "") {
      if (data.length)
        onMessage({
          event: event || "message",
          data: data.join("\n"),
          id,
          retry,
        });
      data = [];
      event = "";
      retry = undefined;
      size = 0;
      return;
    }
    if (value.startsWith(":")) return;
    const colon = value.indexOf(":");
    const field = colon < 0 ? value : value.slice(0, colon);
    let content = colon < 0 ? "" : value.slice(colon + 1);
    if (content.startsWith(" ")) content = content.slice(1);
    if (field === "data") {
      data.push(content);
      size += content.length;
    }
    if (field === "event") event = content;
    if (field === "id" && !content.includes("\0")) id = content;
    if (field === "retry" && /^\d+$/.test(content)) retry = Number(content);
  }
  return {
    feed(chunk: string) {
      if (!chunk) return;
      if (first) {
        chunk = chunk.replace(/^\uFEFF/, "");
        first = false;
      }
      buffer += chunk;
      let start = 0;
      for (let index = 0; index < buffer.length; index++) {
        const char = buffer[index];
        if (char !== "\r" && char !== "\n") continue;
        if (char === "\r" && index === buffer.length - 1) break;
        line(buffer.slice(start, index));
        if (char === "\r" && buffer[index + 1] === "\n") index++;
        start = index + 1;
        if (size > maxBytes)
          throw new Error("Événement temps réel trop volumineux.");
      }
      buffer = buffer.slice(start);
      if (buffer.length + size > maxBytes)
        throw new Error("Événement temps réel trop volumineux.");
    },
  };
}

export async function consumeAuditStream(
  body: ReadableStream<Uint8Array>,
  cursor: number,
  onEvent: (event: AuditEvent) => void,
): Promise<number> {
  const decoder = new TextDecoder();
  const parser = createSseParser((message) => {
    if (message.event !== "audit") return;
    let value: unknown;
    try {
      value = JSON.parse(message.data);
    } catch {
      return;
    }
    if (!isAuditEvent(value) || value.seq <= cursor) return;
    onEvent(value);
    cursor = value.seq;
  });
  const reader = body.getReader();
  try {
    for (;;) {
      const { value, done } = await reader.read();
      if (done) {
        parser.feed(decoder.decode());
        break;
      }
      parser.feed(decoder.decode(value, { stream: true }));
    }
  } finally {
    reader.releaseLock();
  }
  return cursor;
}
