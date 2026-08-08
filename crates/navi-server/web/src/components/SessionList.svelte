<script lang="ts">
  import {
    sessions,
    savedSessions,
    activeSession,
    showSidebar,
    clearChat,
    messages,
    eventsToMessages,
  } from "../lib/stores";
  import {
    listSessions,
    listSavedSessions,
    startSession,
    loadSavedSession,
    deleteSavedSession,
    getSessionSnapshot,
  } from "../lib/api";
  import type { SessionInfo } from "../lib/types";

  let {
    onLogout,
    serverOnline,
  }: { onLogout: () => void; serverOnline: boolean } = $props();

  let loading = $state(false);
  let localError = $state("");

  async function refresh() {
    loading = true;
    localError = "";
    try {
      const [active, saved] = await Promise.all([
        listSessions(),
        listSavedSessions(),
      ]);
      sessions.set(active);
      savedSessions.set(saved);
    } catch (err) {
      localError = err instanceof Error ? err.message : "Failed to load sessions";
    } finally {
      loading = false;
    }
  }

  async function handleNewSession() {
    loading = true;
    localError = "";
    try {
      const info = await startSession();
      await selectSession(info, null);
    } catch (err) {
      localError = err instanceof Error ? err.message : "Failed to create session";
    } finally {
      loading = false;
    }
  }

  async function handleLoadSaved(id: string) {
    loading = true;
    localError = "";
    try {
      const info = await loadSavedSession(id);
      // loadSavedSession returns SessionInfo with snapshot embedded
      await selectSession(info, info.snapshot ?? null);
    } catch (err) {
      localError = err instanceof Error ? err.message : "Failed to load session";
    } finally {
      loading = false;
    }
  }

  async function handleSelectActive(sid: string) {
    loading = true;
    localError = "";
    try {
      // Fetch snapshot for active session to restore history
      const snapshot = await getSessionSnapshot(sid);
      await selectSession({ id: sid }, snapshot);
    } catch (err) {
      // If snapshot fails (e.g. session just created), start with empty chat
      await selectSession({ id: sid }, null);
    } finally {
      loading = false;
    }
  }

  async function handleDeleteSaved(id: string, e: Event) {
    e.stopPropagation();
    try {
      await deleteSavedSession(id);
      await refresh();
    } catch (err) {
      localError = err instanceof Error ? err.message : "Failed to delete session";
    }
  }

  async function selectSession(info: SessionInfo, snapshot: { events: import("../lib/types").AgentEvent[] } | null) {
    clearChat();
    activeSession.set(info);
    showSidebar.set(false);

    // Populate chat from snapshot events if available
    if (snapshot && snapshot.events && snapshot.events.length > 0) {
      const chatMessages = eventsToMessages(snapshot.events);
      messages.set(chatMessages);
    }

    await refresh();
  }

  // Load on mount.
  $effect(() => {
    refresh();
  });
</script>

<div class="session-list">
  <div class="header">
    <h2>NAVI</h2>
    <button class="btn-secondary btn-sm" onclick={onLogout}>Logout</button>
  </div>

  <div class="status-bar">
    <span class="dot" class:online={serverOnline} class:offline={!serverOnline}></span>
    <span class="text-sm text-muted">
      {serverOnline ? "Connected" : "Offline"}
    </span>
  </div>

  <button class="btn-primary new-session" onclick={handleNewSession} disabled={loading}>
    + New Session
  </button>

  {#if localError}
    <p class="error text-sm">{localError}</p>
  {/if}

  <!-- Active sessions -->
  {#if $sessions.length > 0}
    <div class="section">
      <h3>Active</h3>
      {#each $sessions as sid}
        <button
          class="session-item"
          class:active={$activeSession?.id === sid}
          onclick={() => handleSelectActive(sid)}
        >
          <span class="truncate">{sid}</span>
        </button>
      {/each}
    </div>
  {/if}

  <!-- Saved sessions -->
  {#if $savedSessions.length > 0}
    <div class="section">
      <h3>Saved</h3>
      {#each $savedSessions as s}
        <div
          class="session-item saved"
          class:active={$activeSession?.id === s.id}
          onclick={() => handleLoadSaved(s.id)}
          role="button"
          tabindex="0"
        >
          <div class="session-info">
            <span class="truncate session-title">{s.title || s.id}</span>
            <span class="text-muted text-sm">{s.message_count} msgs</span>
          </div>
          <button
            class="btn-delete"
            onclick={(e) => handleDeleteSaved(s.id, e)}
            aria-label="Delete session"
          >
            ×
          </button>
        </div>
      {/each}
    </div>
  {/if}

  {#if !$sessions.length && !$savedSessions.length && !loading}
    <p class="empty text-muted text-sm">No sessions yet.</p>
  {/if}
</div>

<style>
  .session-list {
    display: flex;
    flex-direction: column;
    padding: 0.75rem;
    gap: 0.75rem;
    height: 100%;
  }

  .header {
    display: flex;
    align-items: center;
    justify-content: space-between;
  }

  h2 {
    font-size: 1.1rem;
    color: var(--accent);
  }

  .btn-sm {
    padding: 0.25rem 0.6rem;
    font-size: 0.8rem;
  }

  .status-bar {
    display: flex;
    align-items: center;
    gap: 0.4rem;
  }

  .dot {
    width: 8px;
    height: 8px;
    border-radius: 50%;
  }

  .dot.online {
    background: var(--success);
  }

  .dot.offline {
    background: var(--text-muted);
  }

  .new-session {
    width: 100%;
    padding: 0.6rem;
  }

  .section {
    display: flex;
    flex-direction: column;
    gap: 0.25rem;
  }

  h3 {
    font-size: 0.75rem;
    text-transform: uppercase;
    color: var(--text-muted);
    margin-top: 0.5rem;
    padding: 0 0.25rem;
  }

  .session-item {
    display: flex;
    align-items: center;
    justify-content: space-between;
    padding: 0.6rem 0.75rem;
    background: transparent;
    border: 1px solid transparent;
    border-radius: var(--radius-sm);
    color: var(--text);
    text-align: left;
    width: 100%;
    cursor: pointer;
  }

  .session-item:hover {
    background: var(--bg-tertiary);
  }

  .session-item.active {
    background: var(--bg-tertiary);
    border-color: var(--accent);
  }

  .session-item.saved {
    cursor: pointer;
  }

  .session-info {
    display: flex;
    flex-direction: column;
    gap: 0.1rem;
    flex: 1;
    min-width: 0;
  }

  .session-title {
    font-size: 0.9rem;
  }

  .btn-delete {
    background: transparent;
    border: none;
    color: var(--text-muted);
    font-size: 1.2rem;
    padding: 0 0.3rem;
    line-height: 1;
  }

  .btn-delete:hover {
    color: var(--danger);
  }

  .error {
    color: var(--danger);
    padding: 0.5rem;
    background: rgba(248, 81, 73, 0.1);
    border-radius: var(--radius-sm);
  }

  .empty {
    text-align: center;
    padding: 1rem;
  }
</style>
