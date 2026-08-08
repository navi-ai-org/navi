<script lang="ts">
  import type { CompactNotification } from "../lib/types";

  let {
    notification,
    onDismiss,
  }: {
    notification: CompactNotification;
    onDismiss: () => void;
  } = $props();

  // Auto-dismiss after 5 seconds for completed/failed
  $effect(() => {
    if (notification.type !== "started") {
      const timer = setTimeout(onDismiss, 5000);
      return () => clearTimeout(timer);
    }
  });
</script>

<div class="compact-toast fade-in-up" data-type={notification.type}>
  <div class="icon">
    {#if notification.type === "started"}
      <span class="spinner"></span>
    {:else if notification.type === "completed"}
      <svg width="16" height="16" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="3"><polyline points="20 6 9 17 4 12"/></svg>
    {:else}
      <svg width="16" height="16" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2"><circle cx="12" cy="12" r="10"/><line x1="12" y1="8" x2="12" y2="12"/><line x1="12" y1="16" x2="12.01" y2="16"/></svg>
    {/if}
  </div>
  <div class="content">
    {#if notification.type === "started"}
      <span class="text-sm">Compacting conversation...</span>
    {:else if notification.type === "completed"}
      <span class="text-sm">Compacted — saved {notification.tokensSaved?.toLocaleString() ?? 0} tokens</span>
      {#if notification.summary}
        <span class="text-xs text-muted">{notification.summary}</span>
      {/if}
    {:else}
      <span class="text-sm">Compaction failed: {notification.reason}</span>
    {/if}
  </div>
  <button class="dismiss" onclick={onDismiss} aria-label="Dismiss">×</button>
</div>

<style>
  .compact-toast {
    display: flex;
    align-items: center;
    gap: 0.6rem;
    margin: 0 1rem 0.5rem;
    padding: 0.6rem 0.8rem;
    border-radius: var(--radius);
    box-shadow: var(--shadow-md);
  }

  .compact-toast[data-type="started"] {
    background: var(--accent-subtle);
    border: 1px solid var(--accent);
    color: var(--accent);
  }

  .compact-toast[data-type="completed"] {
    background: var(--success-subtle);
    border: 1px solid var(--success);
    color: var(--success);
  }

  .compact-toast[data-type="failed"] {
    background: var(--danger-subtle);
    border: 1px solid var(--danger);
    color: var(--danger);
  }

  .icon {
    display: flex;
    align-items: center;
    justify-content: center;
    flex-shrink: 0;
  }

  .content {
    flex: 1;
    display: flex;
    flex-direction: column;
    gap: 0.1rem;
  }

  .dismiss {
    background: transparent;
    border: none;
    color: inherit;
    font-size: 1.2rem;
    padding: 0 0.3rem;
    line-height: 1;
    opacity: 0.6;
  }

  .dismiss:hover {
    opacity: 1;
  }
</style>
