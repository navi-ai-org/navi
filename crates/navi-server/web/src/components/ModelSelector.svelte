<script lang="ts">
  import { models, activeSession } from "../lib/stores";
  import { listModels, setSessionModel } from "../lib/api";
  import type { ModelInfo } from "../lib/types";

  let loaded = $state(false);
  let localError = $state("");
  let loadingModels = $state(false);

  // Current selection: "provider/name"
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

    try {
      await setSessionModel($activeSession.id, provider, name);
      activeSession.update((s) => (s ? { ...s, provider, model: name } : s));
    } catch (err) {
      localError = err instanceof Error ? err.message : "Failed to set model";
    }
  }

  // Load models on mount
  $effect(() => {
    loadModels();
  });

  function modelLabel(m: ModelInfo): string {
    return m.label ?? `${m.provider}/${m.name}`;
  }
</script>

<div class="model-selector">
  <select
    value={currentSelection}
    onchange={handleChange}
    disabled={loadingModels || !$activeSession}
    aria-label="Select model"
  >
    {#if !loaded && !loadingModels}
      <option value="">Loading models...</option>
    {:else if $models.length === 0}
      <option value="">No models available</option>
    {:else}
      <option value="" disabled>Select a model...</option>
      {#each $models as m}
        <option value={`${m.provider}/${m.name}`}>
          {modelLabel(m)}
        </option>
      {/each}
    {/if}
  </select>
  {#if localError}
    <span class="error text-sm">{localError}</span>
  {/if}
</div>

<style>
  .model-selector {
    display: flex;
    flex-direction: column;
    gap: 0.2rem;
  }

  select {
    background: var(--bg-secondary);
    color: var(--text);
    border: 1px solid var(--border);
    border-radius: var(--radius-sm);
    padding: 0.3rem 0.5rem;
    font-size: 0.8rem;
    cursor: pointer;
    outline: none;
    max-width: 180px;
  }

  select:focus {
    border-color: var(--accent);
  }

  select:disabled {
    opacity: 0.5;
    cursor: not-allowed;
  }

  .error {
    color: var(--danger);
  }
</style>
