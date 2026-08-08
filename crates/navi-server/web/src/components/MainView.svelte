<script lang="ts">
  import {
    activeSession,
    showSidebar,
    wsStatus,
    error,
  } from "../lib/stores";
  import SessionList from "./SessionList.svelte";
  import ChatView from "./ChatView.svelte";

  let {
    onLogout,
    serverOnline,
  }: { onLogout: () => void; serverOnline: boolean } = $props();

  function toggleSidebar() {
    showSidebar.update((v) => !v);
  }
</script>

<div class="main-view">
  <!-- Sidebar overlay (mobile) -->
  {#if $showSidebar}
    <div class="sidebar-overlay" onclick={() => showSidebar.set(false)}></div>
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
      <div class="empty-state">
        <p class="text-muted">Select or create a session to start chatting.</p>
      </div>
    {/if}
  </div>

  <!-- Top bar (mobile) -->
  {#if $activeSession}
    <div class="topbar">
      <button class="btn-secondary topbar-btn" onclick={toggleSidebar}>
        <svg width="20" height="20" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2">
          <line x1="3" y1="6" x2="21" y2="6" />
          <line x1="3" y1="12" x2="21" y2="12" />
          <line x1="3" y1="18" x2="21" y2="18" />
        </svg>
      </button>
      <span class="topbar-title truncate">
        {$activeSession.title ?? $activeSession.id}
      </span>
      <span class="ws-status-dot" data-status={$wsStatus}></span>
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
      transition: transform 0.2s ease;
    }

    .sidebar.open {
      transform: translateX(0);
    }

    .sidebar-overlay {
      position: absolute;
      inset: 0;
      background: rgba(0, 0, 0, 0.5);
      z-index: 99;
    }
  }

  .chat-area {
    flex: 1;
    display: flex;
    flex-direction: column;
    overflow: hidden;
    position: relative;
  }

  .empty-state {
    flex: 1;
    display: flex;
    align-items: center;
    justify-content: center;
    padding: 2rem;
    text-align: center;
  }

  .topbar {
    position: absolute;
    top: 0;
    left: 0;
    right: 0;
    height: 44px;
    background: var(--bg-secondary);
    border-bottom: 1px solid var(--border);
    display: none;
    align-items: center;
    gap: 0.5rem;
    padding: 0 0.5rem;
    z-index: 10;
  }

  .topbar-btn {
    padding: 0.3rem;
    background: transparent;
    border: none;
    color: var(--text);
  }

  .topbar-title {
    flex: 1;
    font-size: 0.9rem;
  }

  .ws-status-dot {
    width: 8px;
    height: 8px;
    border-radius: 50%;
    background: var(--text-muted);
  }

  .ws-status-dot[data-status="connected"] {
    background: var(--success);
  }

  .ws-status-dot[data-status="connecting"],
  .ws-status-dot[data-status="reconnecting"] {
    background: var(--warning);
  }

  .ws-status-dot[data-status="disconnected"] {
    background: var(--danger);
  }

  @media (max-width: 768px) {
    .topbar {
      display: flex;
    }

    .chat-area {
      padding-top: 44px;
    }
  }
</style>
