import assert from "node:assert/strict";
import { test } from "node:test";
import { createSseParser, consumeAuditStream } from "../src/sse.ts";

test("reconstitue un événement coupé au milieu des champs et de CRLF", () => {
  const messages = [];
  const parser = createSseParser((message) => messages.push(message));
  parser.feed(": maintien\r\nevent: au");
  parser.feed("dit\r");
  parser.feed("\nid: 42\r\ndata: première ligne\r\ndata: seconde ligne\r");
  assert.equal(messages.length, 0);
  parser.feed("\n\r");
  parser.feed("\n");
  assert.deepEqual(messages, [
    {
      event: "audit",
      data: "première ligne\nseconde ligne",
      id: "42",
      retry: undefined,
    },
  ]);
});

test("ignore les commentaires, les événements incomplets et les identifiants NUL", () => {
  const messages = [];
  const parser = createSseParser((message) => messages.push(message));
  parser.feed("\uFEFF: commentaire\n\nid: 7\ndata: un\n\n");
  parser.feed("id: invalide\0\ndata: deux\n\nretry: 1500\ndata: trois\n\n");
  parser.feed("data: incomplet");
  assert.deepEqual(
    messages.map(({ id, data, retry }) => ({ id, data, retry })),
    [
      { id: "7", data: "un", retry: undefined },
      { id: "7", data: "deux", retry: undefined },
      { id: "7", data: "trois", retry: 1500 },
    ],
  );
});

test("le flux UTF-8 saute les répétitions et les messages invalides sans perdre le curseur", async () => {
  const received = [];
  const event = (seq, id = `evt-${seq}`) => ({
    seq,
    id,
    timestamp: "2026-10-29T12:00:00Z",
    kind: "command.completed",
    data: { texte: "écriture" },
  });
  const text = [
    `id: 4\nevent: audit\ndata: ${JSON.stringify(event(4))}\n\n`,
    `id: 5\nevent: audit\ndata: ${JSON.stringify(event(5))}\n\n`,
    "id: 6\nevent: audit\ndata: {invalide}\n\n",
    'id: 7\nevent: audit\ndata: {"seq":7,"kind":"x"}\n\n',
    `id: 8\nevent: audit\ndata: ${JSON.stringify(event(8))}\n\n`,
    `id: 9\nevent: autre\ndata: ${JSON.stringify(event(9))}\n\n`,
  ].join("");
  const bytes = new TextEncoder().encode(text);
  const stream = new ReadableStream({
    start(controller) {
      for (const byte of bytes) controller.enqueue(new Uint8Array([byte]));
      controller.close();
    },
  });
  const cursor = await consumeAuditStream(stream, 4, (item) =>
    received.push(item),
  );
  assert.equal(cursor, 8);
  assert.deepEqual(
    received.map((item) => item.seq),
    [5, 8],
  );
  assert.equal(received[0].data.texte, "écriture");
});

test("refuse un message dont la taille dépasse la limite pour borner la mémoire", () => {
  const parser = createSseParser(() => {}, 64);
  assert.throws(() => parser.feed(`data: ${"a".repeat(65)}`), /volumineux/i);
});
