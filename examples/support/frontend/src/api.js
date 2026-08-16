/** JSON face over the Harbor Fastify process. SQL never leaves the server. */

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
      "Harbor backend is not running. From the repo root run: pnpm example:support"
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

export function ask(payload) {
  return request("/api/ask", { method: "POST", body: JSON.stringify(payload) });
}

export function remember(payload) {
  return request("/api/remember", {
    method: "POST",
    body: JSON.stringify(payload),
  });
}

export function brief(payload) {
  return request("/api/brief", { method: "POST", body: JSON.stringify(payload) });
}

export function digest(payload) {
  return request("/api/digest", {
    method: "POST",
    body: JSON.stringify(payload),
  });
}

export function classify(payload) {
  return request("/api/classify", {
    method: "POST",
    body: JSON.stringify(payload),
  });
}

export async function listTickets() {
  const body = await request("/api/tickets");
  return body.tickets ?? [];
}

export async function listWaiting() {
  const body = await request("/api/waiting");
  return body.waiting ?? [];
}

export function resume(payload) {
  return request("/api/resume", {
    method: "POST",
    body: JSON.stringify(payload),
  });
}

export async function listRuns() {
  const body = await request("/api/runs");
  return body.runs ?? [];
}

export async function listTurns(session) {
  const body = await request(`/api/turns?session=${encodeURIComponent(session)}`);
  return body.turns ?? [];
}
