<script lang="ts">
  import { checkHealth } from "../lib/api";

  let {
    onLogin,
    serverOnline = false,
  }: { onLogin: (secret: string) => void; serverOnline: boolean } = $props();

  let secretInput = $state("");
  let error = $state("");
  let checking = $state(false);
  let showSecret = $state(false);
  let inputEl: HTMLInputElement | null = $state(null);

  // Auto-focus the input on mount
  $effect(() => {
    inputEl?.focus();
  });

  async function handleSubmit(e: Event) {
    e.preventDefault();
    if (!secretInput.trim()) {
      error = "Please enter the server secret.";
      inputEl?.focus();
      return;
    }
    checking = true;
    error = "";

    localStorage.setItem("navi-secret", secretInput.trim());
    const ok = await checkHealth();
    localStorage.removeItem("navi-secret");

    checking = false;
    if (!ok) {
      error = "Cannot reach server. Check the URL and try again.";
      inputEl?.focus();
      return;
    }
    onLogin(secretInput.trim());
  }

  function clearError() {
    if (error) error = "";
  }
</script>

<div class="login-screen">
  <div class="login-card fade-in-up">
    <div class="logo">
      <svg width="56" height="56" viewBox="0 0 32 32">
        <rect width="32" height="32" rx="7" fill="#161b22" stroke="#30363d" />
        <text
          x="16"
          y="22"
          font-family="system-ui, sans-serif"
          font-size="18"
          font-weight="bold"
          fill="#58a6ff"
          text-anchor="middle">N</text
        >
      </svg>
    </div>
    <h1>NAVI</h1>
    <p class="subtitle">Connect to your NAVI server</p>

    <form onsubmit={handleSubmit}>
      <div class="input-wrapper">
        <input
          bind:this={inputEl}
          type={showSecret ? "text" : "password"}
          placeholder="Server secret"
          bind:value={secretInput}
          oninput={clearError}
          autocomplete="off"
          spellcheck="false"
          aria-label="Server secret"
          aria-describedby={error ? "login-error" : undefined}
        />
        <button
          type="button"
          class="toggle-btn"
          onclick={() => showSecret = !showSecret}
          aria-label={showSecret ? "Hide secret" : "Show secret"}
          tabindex="-1"
        >
          {#if showSecret}
            <svg width="18" height="18" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round">
              <path d="M17.94 17.94A10.07 10.07 0 0 1 12 20c-7 0-11-8-11-8a18.45 18.45 0 0 1 5.06-5.94M9.9 4.24A9.12 9.12 0 0 1 12 4c7 0 11 8 11 8a18.5 18.5 0 0 1-2.16 3.19m-6.72-1.07a3 3 0 1 1-4.24-4.24"/>
              <line x1="1" y1="1" x2="23" y2="23"/>
            </svg>
          {:else}
            <svg width="18" height="18" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round">
              <path d="M1 12s4-8 11-8 11 8 11 8-4 8-11 8-11-8-11-8z"/>
              <circle cx="12" cy="12" r="3"/>
            </svg>
          {/if}
        </button>
      </div>

      {#if error}
        <p id="login-error" class="error fade-in" role="alert">
          <svg width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2">
            <circle cx="12" cy="12" r="10"/>
            <line x1="12" y1="8" x2="12" y2="12"/>
            <line x1="12" y1="16" x2="12.01" y2="16"/>
          </svg>
          {error}
        </p>
      {/if}

      <button type="submit" class="btn-primary" disabled={checking || !secretInput.trim()}>
        {#if checking}
          <span class="spinner"></span>
          Connecting...
        {:else}
          Connect
        {/if}
      </button>
    </form>

    <div class="status">
      <span
        class="dot"
        class:online={serverOnline}
        class:offline={!serverOnline}
        class:pulsing={checking}
      ></span>
      <span>{checking ? "Connecting..." : serverOnline ? "Server online" : "Server offline"}</span>
    </div>
  </div>
</div>

<style>
  .login-screen {
    height: 100%;
    display: flex;
    align-items: center;
    justify-content: center;
    padding: 1rem;
    background: radial-gradient(ellipse at top, var(--bg-secondary), var(--bg));
  }

  .login-card {
    width: 100%;
    max-width: 380px;
    background: var(--bg-secondary);
    border: 1px solid var(--border);
    border-radius: var(--radius);
    padding: 2.5rem 2rem;
    text-align: center;
    box-shadow: var(--shadow-lg);
  }

  .logo {
    margin-bottom: 1.25rem;
    display: flex;
    justify-content: center;
  }

  h1 {
    font-size: 1.75rem;
    font-weight: 700;
    color: var(--text);
    margin-bottom: 0.25rem;
    letter-spacing: -0.02em;
  }

  .subtitle {
    color: var(--text-muted);
    font-size: 0.9rem;
    margin-bottom: 2rem;
  }

  form {
    display: flex;
    flex-direction: column;
    gap: 0.75rem;
  }

  .input-wrapper {
    position: relative;
    display: flex;
    align-items: center;
  }

  .input-wrapper input {
    width: 100%;
    padding-right: 2.5rem;
  }

  .toggle-btn {
    position: absolute;
    right: 0.5rem;
    background: transparent;
    border: none;
    color: var(--text-faint);
    padding: 0.3rem;
    cursor: pointer;
    display: flex;
    align-items: center;
    justify-content: center;
    border-radius: var(--radius-xs);
  }

  .toggle-btn:hover {
    color: var(--text-muted);
  }

  button[type="submit"] {
    width: 100%;
    padding: 0.8rem;
    font-size: 1rem;
    display: flex;
    align-items: center;
    justify-content: center;
    gap: 0.5rem;
  }

  .error {
    color: var(--danger);
    font-size: 0.85rem;
    text-align: left;
    display: flex;
    align-items: center;
    gap: 0.4rem;
    padding: 0.5rem 0.7rem;
    background: var(--danger-subtle);
    border-radius: var(--radius-sm);
  }

  .status {
    margin-top: 1.75rem;
    display: flex;
    align-items: center;
    justify-content: center;
    gap: 0.5rem;
    font-size: 0.8rem;
    color: var(--text-muted);
  }

  .dot {
    width: 8px;
    height: 8px;
    border-radius: 50%;
    transition: background var(--transition);
  }

  .dot.online {
    background: var(--success);
    box-shadow: 0 0 6px var(--success);
  }

  .dot.offline {
    background: var(--text-faint);
  }

  .dot.pulsing {
    background: var(--warning);
    animation: pulse 1s infinite;
  }
</style>
