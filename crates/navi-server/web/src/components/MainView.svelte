<script lang="ts">
  import {
    activeSession,
    showSidebar,
    wsStatus,
    error,
  } from "../lib/stores";
  import SessionList from "./SessionList.svelte";
  import ChatView from "./ChatView.svelte";
  import ModelSelector from "./ModelSelector.svelte";

  let {
    onLogout,
    serverOnline,
  }: { onLogout: () => void; serverOnline: boolean } = $props();

  function toggleSidebar() {
    showSidebar.update((v) => !v);
  }

  const wsStatusLabel: Record<string, string> = {
    connected: "Live",
    connecting: "Connecting",
    reconnecting: "Reconnecting",
    disconnected: "Offline",
  };
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
    {#if $activeSession}
      <ChatView />
    {:else}
      <div class="empty-state fade-in">
        <svg width="56" height="56" viewBox="0 0 24 24" fill="none" stroke="var(--text-faint)" stroke-width="1.5">
          <path d="M21 15a2 2 0 0 1-2 2H7l-4 4V5a2 2 0 0 1 2-2h14a2 2 0 0 1 2 2z"/>
        </svg>
        <p class="text-muted text-lg">Welcome to NAVI</p>
        <p class="text-faint text-sm">Select or create a session to start</p>
      </div>
    {/if}
  </div>

  <!-- Top bar (always visible when session active) -->
  {#if $activeSession}
    <div class="topbar safe-top">
      <button
        class="btn-ghost topbar-btn"
        onclick={toggleSidebar}
        aria-label="Toggle sidebar"
      >
        <svg width="20" height="20" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round">
          <line x1="3" y1="6" x2="21" y2="6" />
          <line x1="3" y1="12" x2="21" y2="12" />
          <line x1="3" y1="18" x2="21" y2="18" />
        </svg>
      </button>
      <span class="topbar-title truncate">
        {$activeSession.title ?? $activeSession.id}
      </span>
      <ModelSelector />
      <div class="ws-indicator" data-status={$wsStatus} title={wsStatusLabel[$wsStatus] ?? ""}>
        <span class="ws-dot"></span>
        <span class="ws-label text-xs">{wsStatusLabel[$wsStatus] ?? ""}</span>
      </div>
    </div>
  {/if}
</div>

<style>
  .main-view {
    height: 100%;
    display: flex;
    position: relative;
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
      position: absolute;
      top: 0;
      left: 0;
      bottom: 0;
      z-index: 100;
      transform: translateX(-100%);
      transition: transform var(--transition-slow);
      box-shadow: var(--shadow-lg);
    }

    .sidebar.open {
      transform: translateX(0);
    }

    .sidebar-overlay {
      position: absolute;
      inset: 0;
      background: rgba(0, 0, 0, 0.55);
      z-index: 99;
      backdrop-filter: blur(2px);
    }
  }

  .chat-area {
    flex: 1;
    display: flex;
    flex-direction: column;
    overflow: hidden;
    position: relative;
    padding-top: 48px;
  }

  .empty-state {
    flex: 1;
    display: flex;
    flex-direction: column;
    align-items: center;
    justify-content: center;
    padding: 2rem;
    text-align: center;
    gap: 0.5rem;
  }

  .topbar {
    position: absolute;
    top: 0;
    left: 0;
    right: 0;
    height: 48px;
    background: var(--bg-secondary);
    border-bottom: 1px solid var(--border);
    display: flex;
    align-items: center;
    gap: 0.5rem;
    padding: 0 0.6rem;
    z-index: 10;
  }

  .topbar-btn {
    padding: 0.35rem;
    display: flex;
    align-items: center;
    justify-content: center;
  }

  .topbar-title {
    flex: 1;
    font-size: 0.9rem;
    font-weight: 500;
    color: var(--text);
  }

  .ws-indicator {
    display: flex;
    align-items: center;
    gap: 0.3rem;
    padding: 0.2rem 0.5rem;
    border-radius: var(--radius-sm);
    background: var(--bg-tertiary);
    transition: background var(--transition);
  }

  .ws-dot {
    width: 7px;
    height: 7px;
    border-radius: 50%;
    background: var(--text-muted);
    transition: background var(--transition), box-shadow var(--transition);
  }

  .ws-indicator[data-status="connected"] .ws-dot {
    background: var(--success);
    box-shadow: 0 0 5px var(--success);
  }

  .ws-indicator[data-status="connecting"] .ws-dot,
  .ws-indicator[data-status="reconnecting"] .ws-dot {
    background: var(--warning);
    animation: pulse 1s infinite;
  }

  .ws-indicator[data-status="disconnected"] .ws-dot {
    background: var(--danger);
  }

  .ws-label {
    color: var(--text-muted);
  }

  @media (max-width: 480px) {
    .ws-label {
      display: none;
    }

    .topbar-title {
      font-size: 0.85rem;
    }
  }
</style>
