<script lang="ts">
  import { tokenUsage } from "../lib/stores";

  function formatTokens(n: number): string {
    if (n >= 1_000_000) return `${(n / 1_000_000).toFixed(1)}M`;
    if (n >= 1_000) return `${(n / 1_000).toFixed(1)}k`;
    return String(n);
  }
</script>

{#if $tokenUsage}
  <div class="token-usage" title="{$tokenUsage.inputTokens} in / {$tokenUsage.outputTokens} out">
    <svg width="13" height="13" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round">
      <path d="M12 2L2 7l10 5 10-5-10-5z"/>
      <path d="M2 17l10 5 10-5"/>
      <path d="M2 12l10 5 10-5"/>
    </svg>
    <span class="text-xs">{formatTokens($tokenUsage.inputTokens + $tokenUsage.outputTokens)}</span>
  </div>
{/if}

<style>
  .token-usage {
    display: flex;
    align-items: center;
    gap: 0.25rem;
    padding: 0.2rem 0.45rem;
    border-radius: var(--radius-sm);
    background: var(--bg-tertiary);
    color: var(--text-muted);
    white-space: nowrap;
  }

  @media (max-width: 480px) {
    .token-usage span {
      display: none;
    }
  }
</style>
