<script lang="ts">
  import { models, activeSession } from "../lib/stores";
  import { listModels, setSessionModel } from "../lib/api";
  import type { ModelInfo } from "../lib/types";

  let loaded = $state(false);
  let localError = $state("");
  let loadingModels = $state(false);
  let changing = $state(false);

  let currentSelection = $derived(
    $activeSession?.provider && $activeSession?.model
      ? `${$activeSession.provider}/${$activeSession.model}`
      : "",
  );

  async function loadModels() {
    if (loaded || loadingModels) return;
    loadingModels = true;
    localError = "";
    try {
      const list = await listModels();
      models.set(list);
      loaded = true;
    } catch (err) {
      localError = err instanceof Error ? err.message : "Failed to load models";
    } finally {
      loadingModels = false;
    }
  }

  async function handleChange(e: Event) {
    const select = e.target as HTMLSelectElement;
    const value = select.value;
    if (!value || !$activeSession) return;

    const [provider, name] = value.split("/");
    if (!provider || !name) return;

    changing = true;
    try {
      await setSessionModel($activeSession.id, provider, name);
      activeSession.update((s) => (s ? { ...s, provider, model: name } : s));
    } catch (err) {
      localError = err instanceof Error ? err.message : "Failed to set model";
    } finally {
      changing = false;
    }
  }

  $effect(() => {
    loadModels();
  });

  function modelLabel(m: ModelInfo): string {
    return m.label ?? `${m.provider}/${m.name}`;
  }
</script>

<div class="model-selector">
  <div class="select-wrapper">
    {#if loadingModels || changing}
      <span class="spinner spinner-tiny overlay-spinner"></span>
    {/if}
    <select
      value={currentSelection}
      onchange={handleChange}
      disabled={loadingModels || changing || !$activeSession}
      aria-label="Select model"
    >
      {#if !loaded && !loadingModels}
        <option value="">Loading...</option>
      {:else if $models.length === 0}
        <option value="">No models</option>
      {:else}
        <option value="" disabled>Model...</option>
        {#each $models as m}
          <option value={`${m.provider}/${m.name}`}>
            {modelLabel(m)}
          </option>
        {/each}
      {/if}
    </select>
    <svg class="chevron" width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round">
      <polyline points="6 9 12 15 18 9"/>
    </svg>
  </div>
  {#if localError}
    <span class="error text-xs">{localError}</span>
  {/if}
</div>

<style>
  .model-selector {
    display: flex;
    flex-direction: column;
    gap: 0.15rem;
  }

  .select-wrapper {
    position: relative;
    display: flex;
    align-items: center;
  }

  select {
    appearance: none;
    -webkit-appearance: none;
    background: var(--bg-tertiary);
    color: var(--text-secondary);
    border: 1px solid var(--border);
    border-radius: var(--radius-sm);
    padding: 0.3rem 1.8rem 0.3rem 0.6rem;
    font-size: 0.78rem;
    cursor: pointer;
    outline: none;
    max-width: 160px;
    transition: border-color var(--transition);
  }

  select:hover:not(:disabled) {
    border-color: var(--text-muted);
  }

  select:focus {
    border-color: var(--accent);
  }

  select:disabled {
    opacity: 0.5;
    cursor: not-allowed;
  }

  .chevron {
    position: absolute;
    right: 0.4rem;
    color: var(--text-muted);
    pointer-events: none;
  }

  .overlay-spinner {
    position: absolute;
    right: 1.8rem;
    color: var(--accent);
  }

  .spinner-tiny {
    width: 11px;
    height: 11px;
    border-width: 1.5px;
  }

  .error {
    color: var(--danger);
  }
</style>
