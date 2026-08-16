/** Local live link to `aidb serve` GET /ws. Same process as POST /sql. */

import { withWsToken } from "@/lib/auth";

export type LiveMessage = {
  type: string;
  source?: string;
  ok?: boolean;
};

export function connectLive(handlers: {
  onStatus: (online: boolean) => void;
  onEvent: (message: LiveMessage) => void;
}): () => void {
  const protocol = window.location.protocol === "https:" ? "wss:" : "ws:";
  const url = withWsToken(`${protocol}//${window.location.host}/ws`);
  let socket: WebSocket | null = null;
  let timer: number | null = null;
  let closed = false;
  let delay = 400;

  function clearTimer() {
    if (timer !== null) {
      window.clearTimeout(timer);
      timer = null;
    }
  }

  function open() {
    if (closed) {
      return;
    }
    socket = new WebSocket(url);
    socket.onopen = () => {
      delay = 400;
      handlers.onStatus(true);
    };
    socket.onmessage = (event) => {
      try {
        const message = JSON.parse(String(event.data)) as LiveMessage;
        handlers.onEvent(message);
      } catch {
        handlers.onEvent({ type: "change" });
      }
    };
    socket.onerror = () => {
      socket?.close();
    };
    socket.onclose = () => {
      handlers.onStatus(false);
      if (closed) {
        return;
      }
      clearTimer();
      timer = window.setTimeout(() => {
        delay = Math.min(delay * 2, 8000);
        open();
      }, delay);
    };
  }

  open();

  return () => {
    closed = true;
    clearTimer();
    socket?.close();
  };
}
