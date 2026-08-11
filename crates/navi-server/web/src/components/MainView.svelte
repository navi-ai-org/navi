<script lang="ts">
  import {
    activeSession,
    showSidebar,
    error,
    clearChat,
  } from "../lib/stores";
  import { startSession } from "../lib/api";
  import SessionList from "./SessionList.svelte";
  import ChatView from "./ChatView.svelte";
  import TokenUsage from "./TokenUsage.svelte";

  let {
    onLogout,
    serverOnline,
  }: { onLogout: () => void; serverOnline: boolean } = $props();

  function toggleSidebar() {
    showSidebar.update((v) => !v);
  }

  async function handleNewSession() {
    try {
      const info = await startSession();
      clearChat();
      activeSession.set(info);
      showSidebar.set(false);
    } catch (err) {
      error.set(err instanceof Error ? err.message : "Failed to create session");
    }
  }

</script>

<div class="main-view">
  <!-- Sidebar overlay (mobile) -->
  {#if $showSidebar}
    <div
      class="sidebar-overlay fade-in"
      onclick={() => showSidebar.set(false)}
      onkeydown={(e) => { if (e.key === "Escape") showSidebar.set(false); }}
      role="button"
      tabindex="-1"
      aria-label="Close sidebar"
    ></div>
  {/if}

  <!-- Sidebar -->
  <div class="sidebar" class:open={$showSidebar}>
    <SessionList {onLogout} {serverOnline} />
  </div>

  <!-- Main chat area -->
  <div class="chat-area">
    <ChatView />
  </div>

  <!-- Top bar (always visible) -->
  <div class="topbar safe-top">
    <button
      class="btn-ghost topbar-btn"
      onclick={toggleSidebar}
      aria-label="Alternar painel lateral"
      title="Painel lateral"
    >
      <svg width="18" height="18" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round">
        <rect x="3" y="3" width="18" height="18" rx="3"/>
        <line x1="9" y1="3" x2="9" y2="21"/>
      </svg>
    </button>

    <button class="topbar-title-dropdown" onclick={toggleSidebar} title="Selecionar ou criar sessão">
      <span class="topbar-title-text truncate">
        {$activeSession ? ($activeSession.title ?? $activeSession.id) : "NAVI · Nova conversa"}
      </span>
      <svg class="chevron-icon" width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round">
        <polyline points="6 9 12 15 18 9"/>
      </svg>
    </button>

    <div class="topbar-controls">
      <button class="pill-btn new-chat-pill-btn" onclick={handleNewSession} title="Nova conversa">
        <svg width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.5" stroke-linecap="round">
          <line x1="12" y1="5" x2="12" y2="19"/>
          <line x1="5" y1="12" x2="19" y2="12"/>
        </svg>
        <span class="new-chat-text">Nova conversa</span>
      </button>

      {#if $activeSession}
        <TokenUsage />
      {/if}
    </div>

  </div>
</div>

<style>
  .main-view {
    height: 100%;
    display: flex;
    position: relative;
    background: var(--bg);
  }

  .sidebar {
    width: 280px;
    min-width: 280px;
    background: var(--bg-secondary);
    border-right: 1px solid var(--border);
    overflow-y: auto;
    display: flex;
    flex-direction: column;
  }

  /* On mobile, sidebar is a drawer */
  @media (max-width: 768px) {
    .sidebar {
      position: fixed;
      top: 0;
      left: 0;
      bottom: 0;
      width: 82%;
      max-width: 320px;
      z-index: 1000;
      transform: translateX(-100%);
      transition: transform var(--transition-slow);
      box-shadow: var(--shadow-lg);
    }

    .sidebar.open {
      transform: translateX(0);
    }

    .sidebar-overlay {
      position: fixed;
      inset: 0;
      background: rgba(0, 0, 0, 0.75);
      z-index: 999;
      backdrop-filter: blur(4px);
    }
  }

  .chat-area {
    flex: 1;
    display: flex;
    flex-direction: column;
    overflow: hidden;
    position: relative;
    padding-top: 52px;
    background: var(--bg);
  }



  .topbar {
    position: absolute;
    top: 0;
    left: 0;
    right: 0;
    height: 52px;
    background: var(--bg);
    border-bottom: 1px solid var(--border);
    display: flex;
    align-items: center;
    gap: 0.35rem;
    padding: 0 0.75rem;
    z-index: 10;
  }

  .topbar-btn {
    padding: 0.4rem;
    display: flex;
    align-items: center;
    justify-content: center;
    border-radius: var(--radius-xs);
    color: var(--text-muted);
    flex-shrink: 0;
  }

  .topbar-btn:hover {
    color: var(--text);
    background: var(--bg-tertiary);
  }

  .topbar-title-dropdown {
    display: flex;
    align-items: center;
    gap: 0.4rem;
    flex: 1;
    min-width: 0;
    background: transparent;
    border: none;
    padding: 0.35rem 0.4rem;
    border-radius: var(--radius-xs);
    cursor: pointer;
    color: var(--text);
    font-size: 0.9rem;
    font-weight: 500;
    max-width: none;
    transition: background-color var(--transition), color var(--transition);
  }

  .topbar-title-dropdown:hover {
    background: var(--bg-secondary);
    color: var(--accent-hover);
  }

  .topbar-title-text {
    flex: 1;
    min-width: 0;
  }

  .chevron-icon {
    color: var(--text-muted);
    flex-shrink: 0;
  }

  .topbar-controls {
    display: flex;
    align-items: center;
    gap: 0.4rem;
    flex-shrink: 0;
    margin-left: auto;
  }

  .pill-btn {
    display: inline-flex;
    align-items: center;
    justify-content: center;
    gap: 0.35rem;
    padding: 0.35rem 0.7rem;
    border-radius: var(--radius-sm);
    font-size: 0.8rem;
    font-weight: 500;
    background: transparent;
    color: var(--text-secondary);
    border: 1px solid var(--border);
    transition: background-color var(--transition), border-color var(--transition), color var(--transition);
  }

  .pill-btn:hover {
    background: var(--bg-secondary);
    border-color: var(--border-bright);
    color: var(--text);
  }

  .new-chat-pill-btn {
    color: var(--accent-hover);
  }

  @media (max-width: 600px) {
    .topbar-title-dropdown {
      max-width: none;
      font-size: 0.84rem;
      padding: 0.3rem 0.4rem;
    }

    .new-chat-text {
      display: none;
    }

    .new-chat-pill-btn {
      padding: 0.4rem;
    }

    .topbar {
      padding: 0 0.5rem;
    }
  }
</style>
