/** JSON / SSE face over the chat Fastify process. SQL never leaves the server. */

async function request(path, options = {}) {
  const init = { ...options };
  if (init.body && !init.headers) {
    init.headers = { "content-type": "application/json" };
  }
  let response;
  try {
    response = await fetch(path, init);
  } catch {
    throw new Error(
      "Chat backend is not running. From the repo root run: pnpm example:chat"
    );
  }
  const body = await response.json().catch(() => ({}));
  if (!response.ok || body.ok === false) {
    throw new Error(body.error ?? `HTTP ${response.status}`);
  }
  return body;
}

export async function probe() {
  try {
    const body = await request("/api/health");
    return body.ok === true ? "ok" : "down";
  } catch {
    return "down";
  }
}

export function getStatus() {
  return request("/api/status");
}

export async function chatStream({ session, text }, { onToken } = {}) {
  let response;
  try {
    response = await fetch("/api/chat", {
      method: "POST",
      headers: {
        "content-type": "application/json",
        accept: "text/event-stream",
      },
      body: JSON.stringify({ session, text, stream: true }),
    });
  } catch {
    throw new Error(
      "Chat backend is not running. From the repo root run: pnpm example:chat"
    );
  }
  if (!response.ok) {
    const body = await response.json().catch(() => ({}));
    throw new Error(body.error ?? `HTTP ${response.status}`);
  }
  const reader = response.body.getReader();
  const decoder = new TextDecoder();
  let buffer = "";
  let doneEvent = null;
  while (true) {
    const { value, done } = await reader.read();
    if (done) {
      break;
    }
    buffer += decoder.decode(value, { stream: true });
    const parts = buffer.split("\n\n");
    buffer = parts.pop() ?? "";
    for (const part of parts) {
      const line = part.split("\n").find((row) => row.startsWith("data: "));
      if (!line) {
        continue;
      }
      let event;
      try {
        event = JSON.parse(line.slice(6));
      } catch {
        continue;
      }
      if (event.type === "token") {
        onToken?.(event);
      } else if (event.type === "error") {
        throw new Error(event.error || "generate failed");
      } else if (event.type === "done") {
        doneEvent = event;
      }
    }
  }
  if (!doneEvent) {
    throw new Error("stream ended before the generate run finished");
  }
  return doneEvent;
}

export function addDocument(payload) {
  return request("/api/documents", {
    method: "POST",
    body: JSON.stringify(payload),
  });
}

export async function listDocuments() {
  const body = await request("/api/documents");
  return body.documents ?? [];
}

export async function listSessions() {
  const body = await request("/api/sessions");
  return body.sessions ?? [];
}

export async function listTurns(session) {
  const body = await request(`/api/turns?session=${encodeURIComponent(session)}`);
  return body.turns ?? [];
}
