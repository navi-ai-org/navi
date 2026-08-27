<script lang="ts">
  import type { ToolCallInfo } from "../lib/types";
  import { formatToolValue, presentationFor } from "../lib/toolPresentation";

  let { call }: { call: ToolCallInfo } = $props();
  let expanded = $state(false);

  let presentation = $derived(presentationFor(call));
  let running = $derived(call.status === "requested" || call.status === "started");
  let failed = $derived(call.status === "failed");
  let hasDetails = $derived(
    presentation.expandable && (call.input !== undefined || call.output !== undefined),
  );

  $effect(() => {
    if (failed) expanded = true;
  });

  function toggle() {
    if (hasDetails) expanded = !expanded;
  }
</script>

<div class="tool-row" class:expanded class:failed data-tone={presentation.tone}>
  <button
    type="button"
    class="tool-row-summary"
    class:expandable={hasDetails}
    onclick={toggle}
    aria-expanded={hasDetails ? expanded : undefined}
  >
    <span class="tool-status" aria-hidden="true">
      {#if running}
        <span class="spinner"></span>
      {:else if failed}
        <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.5"><line x1="18" y1="6" x2="6" y2="18"/><line x1="6" y1="6" x2="18" y2="18"/></svg>
      {:else}
        <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.5"><polyline points="20 6 9 17 4 12"/></svg>
      {/if}
    </span>
    <span class="tool-semantic-text">
      <span class="tool-verb">{presentation.verb}</span>
      <span class="tool-target">{presentation.target}</span>
      {#if presentation.detail}
        <span class="tool-detail">{presentation.detail}</span>
      {/if}
    </span>
    <span class="tool-state" class:running class:failed>
      {#if running}Executando{:else if failed}Falhou{:else}Concluído{/if}
    </span>
    <span class="tool-chevron" class:visible={hasDetails} aria-hidden="true">›</span>
  </button>

  {#if expanded && hasDetails}
    <div class="tool-details">
      {#if call.input !== undefined}
        <div class="detail-section">
          <span class="detail-label">Entrada</span>
          <pre>{formatToolValue(call.input)}</pre>
        </div>
      {/if}
      {#if call.output !== undefined}
        <div class="detail-section">
          <span class="detail-label">Resultado</span>
          <pre>{formatToolValue(call.output)}</pre>
        </div>
      {/if}
    </div>
  {/if}
</div>

<style>
  .tool-row {
    width: 100%;
    min-width: 0;
    border-left: 2px solid var(--border);
  }

  .tool-row[data-tone="inspect"] { border-left-color: var(--text-muted); }
  .tool-row[data-tone="change"] { border-left-color: var(--accent); }
  .tool-row[data-tone="command"] { border-left-color: var(--warning); }
  .tool-row[data-tone="browser"] { border-left-color: var(--success); }
  .tool-row.failed { border-left-color: var(--danger); }

  .tool-row-summary {
    display: grid;
    grid-template-columns: 1rem minmax(0, 1fr) auto 0.9rem;
    align-items: center;
    column-gap: 0.55rem;
    width: 100%;
    min-width: 0;
    padding: 0.42rem 0.55rem;
    background: transparent;
    border: none;
    border-radius: 0;
    color: var(--text-secondary);
    text-align: left;
    cursor: default;
  }

  .tool-row-summary.expandable {
    cursor: pointer;
  }

  .tool-row-summary.expandable:hover {
    background: var(--bg-secondary);
  }

  .tool-status {
    display: flex;
    align-items: center;
    justify-content: center;
    width: 1rem;
    height: 1rem;
    color: var(--success);
  }

  .tool-row[data-tone="inspect"] .tool-status { color: var(--text-muted); }
  .tool-row[data-tone="change"] .tool-status { color: var(--accent); }
  .tool-row[data-tone="command"] .tool-status { color: var(--warning); }
  .tool-row[data-tone="browser"] .tool-status { color: var(--success); }
  .tool-row.failed .tool-status { color: var(--danger); }

  .tool-status svg {
    width: 0.95rem;
    height: 0.95rem;
  }

  .tool-status .spinner {
    width: 0.8rem;
    height: 0.8rem;
    border: 1.5px solid currentColor;
    border-top-color: transparent;
  }

  .tool-semantic-text {
    display: flex;
    align-items: baseline;
    gap: 0.5rem;
    min-width: 0;
    overflow: hidden;
    line-height: 1.35;
  }

  .tool-verb {
    flex: 0 0 auto;
    color: var(--text-muted);
    font-size: 0.76rem;
  }

  .tool-target {
    min-width: 0;
    overflow: hidden;
    color: var(--text);
    font-family: var(--font-mono);
    font-size: 0.78rem;
    text-overflow: ellipsis;
    white-space: nowrap;
  }

  .tool-detail {
    flex: 0 0 auto;
    overflow: hidden;
    color: var(--text-faint);
    font-size: 0.7rem;
    text-overflow: ellipsis;
    white-space: nowrap;
  }

  .tool-state {
    min-width: 4.6rem;
    color: var(--text-faint);
    font-size: 0.68rem;
    text-align: right;
    white-space: nowrap;
  }

  .tool-state.running { color: var(--accent); }
  .tool-state.failed { color: var(--danger); }

  .tool-chevron {
    color: var(--text-faint);
    font-size: 1rem;
    line-height: 1;
    opacity: 0;
    transform: translateX(-2px);
    transition: opacity var(--transition), transform var(--transition);
  }

  .tool-chevron.visible {
    opacity: 0.8;
  }

  .tool-row-summary:hover .tool-chevron.visible {
    transform: translateX(0);
  }

  .tool-details {
    display: grid;
    gap: 0.55rem;
    padding: 0.25rem 0.65rem 0.65rem 2.2rem;
  }

  .detail-section {
    min-width: 0;
  }

  .detail-label {
    display: block;
    margin-bottom: 0.2rem;
    color: var(--text-muted);
    font-size: 0.68rem;
    text-transform: uppercase;
    letter-spacing: 0.06em;
  }

  pre {
    max-height: 180px;
    overflow: auto;
    margin: 0;
    padding: 0.5rem;
    background: var(--bg-secondary);
    border-left: 1px solid var(--border);
    color: var(--text-muted);
    font-family: var(--font-mono);
    font-size: 0.7rem;
    line-height: 1.45;
    white-space: pre-wrap;
    overflow-wrap: anywhere;
  }

  @media (max-width: 520px) {
    .tool-row-summary {
      grid-template-columns: 0.9rem minmax(0, 1fr) 0.8rem;
      column-gap: 0.45rem;
      padding: 0.5rem 0.45rem;
    }

    .tool-state {
      display: none;
    }

    .tool-semantic-text {
      display: block;
    }

    .tool-verb {
      margin-right: 0.4rem;
    }

    .tool-detail {
      display: none;
    }

    .tool-details {
      padding-left: 1.8rem;
    }
  }
</style>
