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
    closeSession,
    renameSession,
  } from "../lib/api";
  import type { SessionInfo } from "../lib/types";

  let {
    onLogout,
    serverOnline,
  }: { onLogout: () => void; serverOnline: boolean } = $props();

  let loading = $state(false);
  let initialLoaded = $state(false);
  let localError = $state("");
  let confirmingDelete = $state<string | null>(null);
  let menuOpen = $state<string | null>(null);
  let renaming = $state<string | null>(null);
  let renameValue = $state("");

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
      initialLoaded = true;
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
      await selectSession(info, info.snapshot ?? null);
    } catch (err) {
      localError = err instanceof Error ? err.message : "Failed to load session";
    } finally {
      loading = false;
    }
  }

  async function handleSelectActive(sid: string) {
    if ($activeSession?.id === sid) {
      showSidebar.set(false);
      return;
    }
    loading = true;
    localError = "";
    try {
      const snapshot = await getSessionSnapshot(sid);
      await selectSession({ id: sid }, snapshot);
    } catch (err) {
      await selectSession({ id: sid }, null);
    } finally {
      loading = false;
    }
  }

  async function handleDeleteSaved(id: string, e: Event) {
    e.stopPropagation();
    if (confirmingDelete === id) {
      // Second click confirms
      try {
        await deleteSavedSession(id);
        confirmingDelete = null;
        if ($activeSession?.id === id) {
          activeSession.set(null);
          clearChat();
        }
        await refresh();
      } catch (err) {
        localError = err instanceof Error ? err.message : "Failed to delete session";
      }
    } else {
      confirmingDelete = id;
    }
  }

  async function handleCloseActive(id: string, e: Event) {
    e.stopPropagation();
    menuOpen = null;
    try {
      await closeSession(id);
      if ($activeSession?.id === id) {
        activeSession.set(null);
        clearChat();
      }
      await refresh();
    } catch (err) {
      localError = err instanceof Error ? err.message : "Failed to close session";
    }
  }

  function startRename(id: string, currentTitle: string, e: Event) {
    e.stopPropagation();
    menuOpen = null;
    renaming = id;
    renameValue = currentTitle;
  }

  async function confirmRename(id: string, e: Event) {
    e.stopPropagation();
    e.preventDefault();
    if (!renameValue.trim()) {
      renaming = null;
      return;
    }
    try {
      await renameSession(id, renameValue.trim());
      if ($activeSession?.id === id) {
        activeSession.update((s) => (s ? { ...s, title: renameValue.trim() } : s));
      }
      renaming = null;
      await refresh();
    } catch (err) {
      localError = err instanceof Error ? err.message : "Failed to rename session";
      renaming = null;
    }
  }

  function toggleMenu(id: string, e: Event) {
    e.stopPropagation();
    menuOpen = menuOpen === id ? null : id;
  }

  async function selectSession(info: SessionInfo, snapshot: { events: import("../lib/types").AgentEvent[] } | null) {
    clearChat();
    activeSession.set(info);
    showSidebar.set(false);
    if (snapshot && snapshot.events && snapshot.events.length > 0) {
      const chatMessages = eventsToMessages(snapshot.events);
      messages.set(chatMessages);
    }
    await refresh();
  }

  function formatRelativeTime(isoDate: string): string {
    try {
      const date = new Date(isoDate);
      const now = Date.now();
      const diff = now - date.getTime();
      const mins = Math.floor(diff / 60000);
      const hours = Math.floor(diff / 3600000);
      const days = Math.floor(diff / 86400000);
      if (mins < 1) return "just now";
      if (mins < 60) return `${mins}m ago`;
      if (hours < 24) return `${hours}h ago`;
      if (days < 7) return `${days}d ago`;
      return date.toLocaleDateString();
    } catch {
      return "";
    }
  }

  $effect(() => {
    refresh();
  });
</script>

<div class="session-list">
  <div class="header">
    <div class="brand">
      <svg width="24" height="24" viewBox="0 0 32 32" class="brand-icon">
        <rect width="32" height="32" rx="7" fill="var(--accent)" />
        <text x="16" y="22" font-family="system-ui, sans-serif" font-size="18" font-weight="bold" fill="#fff" text-anchor="middle">N</text>
      </svg>
      <h2>NAVI</h2>
    </div>
    <button class="btn-ghost btn-sm" onclick={onLogout} aria-label="Logout">
      <svg width="18" height="18" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round">
        <path d="M9 21H5a2 2 0 0 1-2-2V5a2 2 0 0 1 2-2h4"/>
        <polyline points="16 17 21 12 16 7"/>
        <line x1="21" y1="12" x2="9" y2="12"/>
      </svg>
    </button>
  </div>

  <div class="status-bar">
    <span class="dot" class:online={serverOnline} class:offline={!serverOnline}></span>
    <span class="text-sm text-muted">
      {serverOnline ? "Connected" : "Offline"}
    </span>
  </div>

  <button class="btn-primary new-session" onclick={handleNewSession} disabled={loading}>
    <svg width="18" height="18" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round">
      <line x1="12" y1="5" x2="12" y2="19"/>
      <line x1="5" y1="12" x2="19" y2="12"/>
    </svg>
    New Session
  </button>

  {#if localError}
    <p class="error text-sm fade-in">
      <svg width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2">
        <circle cx="12" cy="12" r="10"/>
        <line x1="12" y1="8" x2="12" y2="12"/>
        <line x1="12" y1="16" x2="12.01" y2="16"/>
      </svg>
      {localError}
    </p>
  {/if}

  <!-- Loading skeleton -->
  {#if !initialLoaded && loading}
    <div class="section">
      <div class="skeleton skeleton-header"></div>
      {#each Array(3) as _, i}
        <div class="skeleton skeleton-item" style="animation-delay: {i * 0.1}s"></div>
      {/each}
    </div>
  {/if}

  <!-- Active sessions -->
  {#if $sessions.length > 0}
    <div class="section">
      <h3>Active <span class="count">{$sessions.length}</span></h3>
      {#each $sessions as sid}
        <div
          class="session-item active-row"
          class:active={$activeSession?.id === sid}
          role="button"
          tabindex="0"
          onclick={() => handleSelectActive(sid)}
          onkeydown={(e) => { if (e.key === "Enter") handleSelectActive(sid); }}
        >
          <div class="session-icon active-icon"></div>
          <span class="truncate session-id">{sid}</span>
          <button class="menu-toggle" onclick={(e) => toggleMenu(sid, e)} aria-label="Session options">
            <svg width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2"><circle cx="12" cy="12" r="1"/><circle cx="12" cy="5" r="1"/><circle cx="12" cy="19" r="1"/></svg>
          </button>
          {#if menuOpen === sid}
            <div class="ctx-menu fade-in-up" role="menu" onclick={(e) => e.stopPropagation()} onkeydown={(e) => e.stopPropagation()}>
              <button class="ctx-item" role="menuitem" onclick={(e) => startRename(sid, sid, e)}>
                <svg width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2"><path d="M11 4H4a2 2 0 0 0-2 2v14a2 2 0 0 0 2 2h14a2 2 0 0 0 2-2v-7"/><path d="M18.5 2.5a2.121 2.121 0 0 1 3 3L12 15l-4 1 1-4 9.5-9.5z"/></svg>
                Rename
              </button>
              <button class="ctx-item danger" role="menuitem" onclick={(e) => handleCloseActive(sid, e)}>
                <svg width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2"><path d="M18 6L6 18M6 6l12 12"/></svg>
                Close
              </button>
            </div>
          {/if}
        </div>
      {/each}
    </div>
  {/if}

  <!-- Saved sessions -->
  {#if $savedSessions.length > 0}
    <div class="section">
      <h3>Saved <span class="count">{$savedSessions.length}</span></h3>
      {#each $savedSessions as s}
        {#if renaming === s.id}
          <form class="rename-form" onsubmit={(e) => confirmRename(s.id, e)}>
            <input bind:value={renameValue} placeholder="New title" />
            <button type="submit" class="btn-primary btn-sm">Save</button>
            <button type="button" class="btn-secondary btn-sm" onclick={(e) => { e.stopPropagation(); renaming = null; }}>Cancel</button>
          </form>
        {:else}
          <div
            class="session-item saved"
            class:active={$activeSession?.id === s.id}
            class:confirming={confirmingDelete === s.id}
            onclick={() => handleLoadSaved(s.id)}
            role="button"
            tabindex="0"
            onkeydown={(e) => { if (e.key === "Enter") handleLoadSaved(s.id); }}
          >
            <div class="session-info">
              <span class="truncate session-title">{s.title || s.id}</span>
              <div class="session-meta">
                <span class="text-xs text-faint">{s.message_count} msgs</span>
                {#if s.updated_at}
                  <span class="text-xs text-faint">· {formatRelativeTime(s.updated_at)}</span>
                {/if}
              </div>
            </div>
            <button class="menu-toggle" onclick={(e) => toggleMenu(s.id, e)} aria-label="Session options">
              <svg width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2"><circle cx="12" cy="12" r="1"/><circle cx="12" cy="5" r="1"/><circle cx="12" cy="19" r="1"/></svg>
            </button>
            {#if menuOpen === s.id}
              <div class="ctx-menu fade-in-up" role="menu" onclick={(e) => e.stopPropagation()} onkeydown={(e) => e.stopPropagation()}>
                <button class="ctx-item" role="menuitem" onclick={(e) => startRename(s.id, s.title || s.id, e)}>
                  <svg width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2"><path d="M11 4H4a2 2 0 0 0-2 2v14a2 2 0 0 0 2 2h14a2 2 0 0 0 2-2v-7"/><path d="M18.5 2.5a2.121 2.121 0 0 1 3 3L12 15l-4 1 1-4 9.5-9.5z"/></svg>
                  Rename
                </button>
                <button class="ctx-item danger" role="menuitem" onclick={(e) => handleDeleteSaved(s.id, e)}>
                  <svg width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2"><polyline points="3 6 5 6 21 6"/><path d="M19 6v14a2 2 0 0 1-2 2H7a2 2 0 0 1-2-2V6m3 0V4a2 2 0 0 1 2-2h4a2 2 0 0 1 2 2v2"/></svg>
                  Delete
                </button>
              </div>
            {/if}
            {#if confirmingDelete === s.id && menuOpen !== s.id}
              <button
                class="btn-delete confirming"
                onclick={(e) => handleDeleteSaved(s.id, e)}
                aria-label="Confirm delete"
              >
                <svg width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2">
                  <polyline points="3 6 5 6 21 6"/>
                  <path d="M19 6v14a2 2 0 0 1-2 2H7a2 2 0 0 1-2-2V6m3 0V4a2 2 0 0 1 2-2h4a2 2 0 0 1 2 2v2"/>
                </svg>
              </button>
            {/if}
          </div>
        {/if}
      {/each}
    </div>
  {/if}

  <!-- Empty state -->
  {#if initialLoaded && !$sessions.length && !$savedSessions.length}
    <div class="empty-state fade-in">
      <svg width="40" height="40" viewBox="0 0 24 24" fill="none" stroke="var(--text-faint)" stroke-width="1.5">
        <path d="M21 15a2 2 0 0 1-2 2H7l-4 4V5a2 2 0 0 1 2-2h14a2 2 0 0 1 2 2z"/>
      </svg>
      <p class="text-muted text-sm">No sessions yet</p>
      <p class="text-faint text-xs">Create one to start chatting</p>
    </div>
  {/if}
</div>

<style>
  .session-list {
    display: flex;
    flex-direction: column;
    padding: 0.75rem;
    gap: 0.75rem;
    height: 100%;
    overflow-y: auto;
  }

  .header {
    display: flex;
    align-items: center;
    justify-content: space-between;
  }

  .brand {
    display: flex;
    align-items: center;
    gap: 0.5rem;
  }

  .brand-icon {
    border-radius: 6px;
  }

  h2 {
    font-size: 1.1rem;
    font-weight: 700;
    color: var(--text);
    letter-spacing: -0.01em;
  }

  .btn-sm {
    padding: 0.3rem;
  }

  .status-bar {
    display: flex;
    align-items: center;
    gap: 0.4rem;
    padding-bottom: 0.25rem;
  }

  .dot {
    width: 8px;
    height: 8px;
    border-radius: 50%;
  }

  .dot.online {
    background: var(--success);
    box-shadow: 0 0 6px var(--success);
  }

  .dot.offline {
    background: var(--text-faint);
  }

  .new-session {
    width: 100%;
    padding: 0.65rem;
    display: flex;
    align-items: center;
    justify-content: center;
    gap: 0.4rem;
    font-weight: 500;
  }

  .section {
    display: flex;
    flex-direction: column;
    gap: 0.2rem;
  }

  h3 {
    font-size: 0.7rem;
    text-transform: uppercase;
    letter-spacing: 0.05em;
    color: var(--text-faint);
    margin-top: 0.5rem;
    padding: 0 0.25rem;
    display: flex;
    align-items: center;
    gap: 0.4rem;
  }

  .count {
    background: var(--bg-tertiary);
    padding: 0.05rem 0.4rem;
    border-radius: 10px;
    font-size: 0.65rem;
    color: var(--text-muted);
  }

  .session-item {
    display: flex;
    align-items: center;
    gap: 0.5rem;
    padding: 0.6rem 0.7rem;
    background: transparent;
    border: 1px solid transparent;
    border-radius: var(--radius-sm);
    color: var(--text-secondary);
    text-align: left;
    width: 100%;
    cursor: pointer;
    transition: background var(--transition), border-color var(--transition);
  }

  .session-item:hover:not(:disabled) {
    background: var(--bg-tertiary);
  }

  .session-item.active {
    background: var(--accent-subtle);
    border-color: var(--accent);
  }

  .session-item.active .session-id {
    color: var(--accent);
  }

  .session-item.confirming {
    background: var(--danger-subtle);
    border-color: var(--danger);
  }

  .session-icon {
    width: 8px;
    height: 8px;
    border-radius: 50%;
    flex-shrink: 0;
  }

  .active-icon {
    background: var(--success);
    box-shadow: 0 0 4px var(--success);
  }

  .session-id {
    font-family: var(--font-mono);
    font-size: 0.8rem;
  }

  .session-item.saved {
    cursor: pointer;
  }

  .session-info {
    display: flex;
    flex-direction: column;
    gap: 0.15rem;
    flex: 1;
    min-width: 0;
  }

  .session-title {
    font-size: 0.9rem;
    font-weight: 500;
  }

  .session-meta {
    display: flex;
    gap: 0.25rem;
    align-items: center;
  }

  .btn-delete {
    background: transparent;
    border: none;
    color: var(--text-faint);
    font-size: 1.2rem;
    padding: 0.2rem 0.4rem;
    line-height: 1;
    border-radius: var(--radius-xs);
    transition: color var(--transition), background var(--transition);
    flex-shrink: 0;
  }

  .btn-delete:hover {
    color: var(--danger);
    background: var(--danger-subtle);
  }

  .btn-delete.confirming {
    color: var(--danger);
    background: var(--danger);
    color: #fff;
  }

  /* Active row with menu */
  .active-row {
    position: relative;
  }

  .active-row .session-id {
    flex: 1;
    min-width: 0;
  }

  .menu-toggle {
    background: transparent;
    border: none;
    color: var(--text-faint);
    padding: 0.2rem;
    border-radius: var(--radius-xs);
    cursor: pointer;
    display: flex;
    align-items: center;
    justify-content: center;
    opacity: 0;
    transition: opacity var(--transition), color var(--transition);
    flex-shrink: 0;
  }

  .session-item:hover .menu-toggle,
  .session-item.active .menu-toggle {
    opacity: 1;
  }

  .menu-toggle:hover {
    color: var(--text);
    background: var(--bg-hover);
  }

  .ctx-menu {
    position: absolute;
    right: 0.5rem;
    top: 100%;
    z-index: 50;
    background: var(--bg-secondary);
    border: 1px solid var(--border);
    border-radius: var(--radius-sm);
    box-shadow: var(--shadow-lg);
    padding: 0.25rem;
    min-width: 140px;
    display: flex;
    flex-direction: column;
    gap: 0.1rem;
  }

  .ctx-item {
    display: flex;
    align-items: center;
    gap: 0.5rem;
    padding: 0.45rem 0.6rem;
    background: transparent;
    border: none;
    border-radius: var(--radius-xs);
    color: var(--text-secondary);
    font-size: 0.82rem;
    cursor: pointer;
    text-align: left;
    width: 100%;
    transition: background var(--transition), color var(--transition);
  }

  .ctx-item:hover {
    background: var(--bg-hover);
    color: var(--text);
  }

  .ctx-item.danger:hover {
    color: var(--danger);
    background: var(--danger-subtle);
  }

  .rename-form {
    display: flex;
    gap: 0.3rem;
    padding: 0.4rem;
    background: var(--bg-tertiary);
    border-radius: var(--radius-sm);
    border: 1px solid var(--accent);
  }

  .rename-form input {
    flex: 1;
    font-size: 0.85rem;
  }

  .error {
    color: var(--danger);
    padding: 0.5rem 0.7rem;
    background: var(--danger-subtle);
    border-radius: var(--radius-sm);
    display: flex;
    align-items: center;
    gap: 0.4rem;
  }

  .empty-state {
    text-align: center;
    padding: 2rem 1rem;
    display: flex;
    flex-direction: column;
    align-items: center;
    gap: 0.4rem;
  }

  /* Skeletons */
  .skeleton-header {
    height: 20px;
    width: 80px;
    margin: 0.5rem 0.25rem;
  }

  .skeleton-item {
    height: 48px;
    width: 100%;
  }
</style>
