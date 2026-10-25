// One frame of the event stream, as the hub writes it (see e2e/fixtures/sse-golden).
export function frame(id: number, event: { type: string } & Record<string, unknown>): string {
  return `id: ${id}\nevent: ${event.type}\ndata: ${JSON.stringify(event)}\n\n`;
}
