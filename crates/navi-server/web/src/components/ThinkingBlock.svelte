<script lang="ts">
  let {
    text,
    streaming = false,
  }: {
    text: string;
    streaming?: boolean;
  } = $props();

  let expanded = $state(false);
  let wasStreaming = $state(false);
  let contentEl: HTMLDivElement | null = $state(null);

  $effect(() => {
    if (streaming) {
      expanded = true;
      queueMicrotask(() => {
        if (contentEl) contentEl.scrollTop = contentEl.scrollHeight;
      });
    } else if (wasStreaming) {
      expanded = false;
    }
    wasStreaming = streaming;
  });

  function toggle() {
    expanded = !expanded;
  }
</script>

{#if text || streaming}
  <div class="thinking-block" class:streaming>
    <button
      type="button"
      class="thinking-toggle"
      onclick={toggle}
      aria-expanded={expanded}
    >
      <span class="thinking-mark" aria-hidden="true">
        {#if streaming}
          <span class="thinking-spinner"></span>
        {:else}
          <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.7"><path d="M9 17h6M10 21h4M8 14a5 5 0 1 1 8 0c-.7.6-1 1.2-1 2H9c0-.8-.3-1.4-1-2Z"/><path d="M12 2v1M4.9 4.9l.7.7M2 12h1M19.1 4.9l-.7.7M21 12h-1"/></svg>
        {/if}
      </span>
      <span class="thinking-label">{streaming ? "Pensando" : "Pensamento"}</span>
      <span class="thinking-chevron" class:expanded aria-hidden="true">›</span>
    </button>

    {#if expanded}
      <div class="thinking-content" bind:this={contentEl}>
        {text}
        {#if streaming}<span class="thinking-cursor" aria-hidden="true"></span>{/if}
      </div>
    {/if}
  </div>
{/if}

<style>
  .thinking-block {
    width: 100%;
    color: var(--text-muted);
  }

  .thinking-toggle {
    display: flex;
    align-items: center;
    gap: 0.45rem;
    width: 100%;
    padding: 0.3rem 0;
    background: transparent;
    border: none;
    color: var(--text-muted);
    font-size: 0.78rem;
    text-align: left;
  }

  .thinking-toggle:hover {
    color: var(--text-secondary);
    background: transparent;
  }

  .thinking-mark {
    display: flex;
    align-items: center;
    justify-content: center;
    width: 1rem;
    height: 1rem;
    color: var(--accent);
  }

  .thinking-mark svg {
    width: 0.95rem;
    height: 0.95rem;
  }

  .thinking-spinner {
    width: 0.75rem;
    height: 0.75rem;
    border: 1.5px solid var(--accent);
    border-top-color: transparent;
    border-radius: 50%;
    animation: spin 0.7s linear infinite;
  }

  .thinking-label {
    flex: 1;
    font-style: italic;
  }

  .thinking-chevron {
    color: var(--text-faint);
    font-size: 1rem;
    line-height: 1;
    transform: translateX(-2px);
    transition: transform var(--transition), color var(--transition);
  }

  .thinking-chevron.expanded {
    transform: rotate(90deg);
  }

  .thinking-content {
    max-height: min(40vh, 320px);
    overflow-y: auto;
    padding: 0.15rem 0 0.45rem 1.45rem;
    color: var(--text-muted);
    font-family: var(--font-mono);
    font-size: 0.73rem;
    font-style: italic;
    line-height: 1.5;
    white-space: pre-wrap;
    overflow-wrap: anywhere;
  }

  .thinking-cursor {
    display: inline-block;
    width: 5px;
    height: 0.9em;
    margin-left: 2px;
    background: var(--accent);
    vertical-align: text-bottom;
    animation: blink 0.8s steps(2) infinite;
  }
</style>
