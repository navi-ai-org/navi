import type { RuntimeEvent } from "./types";
import { getSecret } from "./api";

// ── WebSocket client ─────────────────────────────────────────────────────
//
// Connects to /sessions/:id/events?secret=... and streams RuntimeEvents.
// Auto-reconnects with exponential backoff on disconnect.

export type EventHandler = (event: RuntimeEvent) => void;
export type StatusHandler = (status: ConnectionStatus) => void;

export type ConnectionStatus =
  | "connecting"
  | "connected"
  | "disconnected"
  | "reconnecting";

export class EventStream {
  private ws: WebSocket | null = null;
  private url: string;
  private handlers: Set<EventHandler> = new Set();
  private statusHandlers: Set<StatusHandler> = new Set();
  private reconnectDelay = 1000;
  private maxReconnectDelay = 30000;
  private shouldReconnect = true;
  private reconnectTimer: ReturnType<typeof setTimeout> | null = null;

  constructor(sessionId: string) {
    const secret = encodeURIComponent(getSecret());
    const protocol = window.location.protocol === "https:" ? "wss:" : "ws:";
    const host = window.location.host;
    this.url = `${protocol}//${host}/sessions/${sessionId}/events?secret=${secret}`;
  }

  connect(): void {
    this.shouldReconnect = true;
    this.setStatus("connecting");
    this.doConnect();
  }

  private doConnect(): void {
    try {
      this.ws = new WebSocket(this.url);
    } catch (err) {
      console.error("WebSocket creation failed:", err);
      this.scheduleReconnect();
      return;
    }

    this.ws.onopen = () => {
      this.reconnectDelay = 1000;
      this.setStatus("connected");
    };

    this.ws.onmessage = (event: MessageEvent) => {
      if (typeof event.data !== "string" || !event.data.trim()) return;
      try {
        const data = JSON.parse(event.data) as RuntimeEvent;
        this.handlers.forEach((h) => h(data));
      } catch (err) {
        console.error("Failed to parse WebSocket message:", err);
      }
    };

    this.ws.onerror = (event: Event) => {
      console.error("WebSocket error:", event);
    };

    this.ws.onclose = () => {
      this.ws = null;
      this.setStatus("disconnected");
      if (this.shouldReconnect) {
        this.scheduleReconnect();
      }
    };
  }

  private scheduleReconnect(): void {
    if (this.reconnectTimer) clearTimeout(this.reconnectTimer);
    this.setStatus("reconnecting");
    this.reconnectTimer = setTimeout(() => {
      this.doConnect();
    }, this.reconnectDelay);
    // Exponential backoff
    this.reconnectDelay = Math.min(
      this.reconnectDelay * 2,
      this.maxReconnectDelay,
    );
  }

  disconnect(): void {
    this.shouldReconnect = false;
    if (this.reconnectTimer) {
      clearTimeout(this.reconnectTimer);
      this.reconnectTimer = null;
    }
    if (this.ws) {
      this.ws.close();
      this.ws = null;
    }
    this.setStatus("disconnected");
  }

  onEvent(handler: EventHandler): () => void {
    this.handlers.add(handler);
    return () => this.handlers.delete(handler);
  }

  onStatus(handler: StatusHandler): () => void {
    this.statusHandlers.add(handler);
    return () => this.statusHandlers.delete(handler);
  }

  private setStatus(status: ConnectionStatus): void {
    this.statusHandlers.forEach((h) => h(status));
  }
}
