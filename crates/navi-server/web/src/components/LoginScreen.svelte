<script lang="ts">
  import { checkHealth } from "../lib/api";

  let {
    onLogin,
    serverOnline = false,
  }: { onLogin: (secret: string) => void; serverOnline: boolean } = $props();

  let secretInput = $state("");
  let error = $state("");
  let checking = $state(false);

  async function handleSubmit(e: Event) {
    e.preventDefault();
    if (!secretInput.trim()) {
      error = "Please enter the server secret.";
      return;
    }
    checking = true;
    error = "";

    // Temporarily set the secret to test the connection.
    localStorage.setItem("navi-secret", secretInput.trim());
    const ok = await checkHealth();
    localStorage.removeItem("navi-secret");

    checking = false;
    if (!ok) {
      error = "Cannot reach server. Check the URL and try again.";
      return;
    }
    onLogin(secretInput.trim());
  }
</script>

<div class="login-screen">
  <div class="login-card">
    <div class="logo">
      <svg width="48" height="48" viewBox="0 0 32 32">
        <rect width="32" height="32" rx="6" fill="#161b22" stroke="#30363d" />
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
      <input
        type="password"
        placeholder="Server secret"
        bind:value={secretInput}
        autocomplete="off"
        spellcheck="false"
      />
      {#if error}
        <p class="error">{error}</p>
      {/if}
      <button type="submit" class="btn-primary" disabled={checking}>
        {checking ? "Connecting..." : "Connect"}
      </button>
    </form>

    <div class="status">
      <span class="dot" class:online={serverOnline} class:offline={!serverOnline}></span>
      <span>{serverOnline ? "Server online" : "Server offline"}</span>
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
  }

  .login-card {
    width: 100%;
    max-width: 360px;
    background: var(--bg-secondary);
    border: 1px solid var(--border);
    border-radius: var(--radius);
    padding: 2rem 1.5rem;
    text-align: center;
  }

  .logo {
    margin-bottom: 1rem;
    display: flex;
    justify-content: center;
  }

  h1 {
    font-size: 1.5rem;
    color: var(--accent);
    margin-bottom: 0.25rem;
  }

  .subtitle {
    color: var(--text-muted);
    font-size: 0.9rem;
    margin-bottom: 1.5rem;
  }

  form {
    display: flex;
    flex-direction: column;
    gap: 0.75rem;
  }

  input {
    width: 100%;
  }

  button {
    width: 100%;
    padding: 0.7rem;
    font-size: 1rem;
  }

  .error {
    color: var(--danger);
    font-size: 0.85rem;
    text-align: left;
  }

  .status {
    margin-top: 1.5rem;
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
  }

  .dot.online {
    background: var(--success);
  }

  .dot.offline {
    background: var(--text-muted);
  }
</style>
