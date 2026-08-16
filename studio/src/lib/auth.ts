const STORAGE_KEY = "aidb.bearer";

export function getBearer(): string | null {
  const stored = window.localStorage.getItem(STORAGE_KEY);
  const trimmed = stored?.trim();
  return trimmed ? trimmed : null;
}

export function setBearer(token: string | null): void {
  const trimmed = token?.trim() ?? "";
  if (!trimmed) {
    window.localStorage.removeItem(STORAGE_KEY);
  } else {
    window.localStorage.setItem(STORAGE_KEY, trimmed);
  }
  window.dispatchEvent(new Event("aidb-auth"));
}

/** First load: `?token=` or `?bearer=` becomes localStorage, then leaves the URL. */
export function takeTokenFromUrl(): boolean {
  const url = new URL(window.location.href);
  const token = url.searchParams.get("token") ?? url.searchParams.get("bearer");
  if (!token?.trim()) {
    return false;
  }
  setBearer(token);
  url.searchParams.delete("token");
  url.searchParams.delete("bearer");
  const next = `${url.pathname}${url.search}${url.hash}`;
  window.history.replaceState(window.history.state, "", next);
  return true;
}

export function authHeaders(): Record<string, string> {
  const token = getBearer();
  return token ? { Authorization: `Bearer ${token}` } : {};
}

export function withWsToken(url: string): string {
  const token = getBearer();
  if (!token) {
    return url;
  }
  const parsed = new URL(url, window.location.origin);
  parsed.searchParams.set("token", token);
  return parsed.toString();
}
