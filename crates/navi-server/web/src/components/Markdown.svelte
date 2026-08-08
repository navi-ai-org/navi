<script lang="ts">
  import { marked } from "marked";

  let { content }: { content: string } = $props();

  marked.setOptions({
    gfm: true,
    breaks: true,
  });

  let html = $derived(marked.parse(content, { async: false }) as string);
  let container: HTMLDivElement | null = $state(null);

  // Add copy buttons to code blocks after render
  $effect(() => {
    // Re-run when html changes
    html;
    queueMicrotask(() => {
      if (!container) return;
      const blocks = container.querySelectorAll("pre > code");
      blocks.forEach((code) => {
        const pre = code.parentElement as HTMLPreElement;
        if (pre.querySelector(".copy-btn")) return;

        const btn = document.createElement("button");
        btn.className = "copy-btn";
        btn.setAttribute("aria-label", "Copy code");
        btn.innerHTML =
          '<svg width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><rect x="9" y="9" width="13" height="13" rx="2" ry="2"/><path d="M5 15H4a2 2 0 0 1-2-2V4a2 2 0 0 1 2-2h9a2 2 0 0 1 2 2v1"/></svg>';
        btn.onclick = async (e) => {
          e.preventDefault();
          const text = code.textContent ?? "";
          try {
            await navigator.clipboard.writeText(text);
            btn.innerHTML =
              '<svg width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="3" stroke-linecap="round" stroke-linejoin="round"><polyline points="20 6 9 17 4 12"/></svg>';
            btn.classList.add("copied");
            setTimeout(() => {
              btn.innerHTML =
                '<svg width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><rect x="9" y="9" width="13" height="13" rx="2" ry="2"/><path d="M5 15H4a2 2 0 0 1-2-2V4a2 2 0 0 1 2-2h9a2 2 0 0 1 2 2v1"/></svg>';
              btn.classList.remove("copied");
            }, 1500);
          } catch {
            // Clipboard not available
          }
        };
        pre.appendChild(btn);
      });
    });
  });
</script>

<!-- eslint-disable svelte/no-vhtml -->
<div class="markdown-body" bind:this={container}>{@html html}</div>

<style>
  .markdown-body {
    font-size: 0.9rem;
    line-height: 1.65;
    word-wrap: break-word;
    overflow-wrap: break-word;
  }

  .markdown-body :global(p) {
    margin: 0.5rem 0;
  }

  .markdown-body :global(p:first-child) {
    margin-top: 0;
  }

  .markdown-body :global(p:last-child) {
    margin-bottom: 0;
  }

  .markdown-body :global(h1),
  .markdown-body :global(h2),
  .markdown-body :global(h3),
  .markdown-body :global(h4),
  .markdown-body :global(h5),
  .markdown-body :global(h6) {
    margin: 1rem 0 0.5rem;
    font-weight: 600;
    line-height: 1.3;
    color: var(--text);
  }

  .markdown-body :global(h1) {
    font-size: 1.3rem;
    border-bottom: 1px solid var(--border);
    padding-bottom: 0.3rem;
  }

  .markdown-body :global(h2) {
    font-size: 1.15rem;
    border-bottom: 1px solid var(--border-subtle);
    padding-bottom: 0.2rem;
  }

  .markdown-body :global(h3) {
    font-size: 1.05rem;
  }

  .markdown-body :global(ul),
  .markdown-body :global(ol) {
    margin: 0.5rem 0;
    padding-left: 1.5rem;
  }

  .markdown-body :global(li) {
    margin: 0.25rem 0;
  }

  .markdown-body :global(li > ul),
  .markdown-body :global(li > ol) {
    margin: 0.25rem 0;
  }

  .markdown-body :global(code) {
    font-family: var(--font-mono);
    font-size: 0.85em;
    background: var(--bg-tertiary);
    padding: 0.15rem 0.35rem;
    border-radius: var(--radius-xs);
    color: var(--accent);
  }

  .markdown-body :global(pre) {
    position: relative;
    background: var(--bg);
    border: 1px solid var(--border);
    border-radius: var(--radius-sm);
    padding: 0.85rem;
    padding-right: 2.5rem;
    overflow-x: auto;
    margin: 0.6rem 0;
  }

  .markdown-body :global(pre code) {
    background: transparent;
    padding: 0;
    font-size: 0.82rem;
    line-height: 1.55;
    color: var(--text-secondary);
  }

  .markdown-body :global(.copy-btn) {
    position: absolute;
    top: 0.5rem;
    right: 0.5rem;
    background: var(--bg-tertiary);
    border: 1px solid var(--border);
    border-radius: var(--radius-xs);
    padding: 0.3rem;
    cursor: pointer;
    color: var(--text-muted);
    display: flex;
    align-items: center;
    justify-content: center;
    opacity: 0;
    transition: opacity var(--transition), color var(--transition),
      background var(--transition);
  }

  .markdown-body :global(pre:hover .copy-btn) {
    opacity: 1;
  }

  .markdown-body :global(.copy-btn:hover) {
    color: var(--text);
    background: var(--bg-hover);
  }

  .markdown-body :global(.copy-btn.copied) {
    color: var(--success);
    opacity: 1;
  }

  .markdown-body :global(blockquote) {
    border-left: 3px solid var(--accent);
    margin: 0.6rem 0;
    padding: 0.3rem 0.9rem;
    color: var(--text-muted);
    background: var(--accent-subtle);
    border-radius: 0 var(--radius-sm) var(--radius-sm) 0;
  }

  .markdown-body :global(blockquote p) {
    margin: 0.25rem 0;
  }

  .markdown-body :global(a) {
    color: var(--accent);
    text-decoration: none;
    border-bottom: 1px solid transparent;
    transition: border-color var(--transition);
  }

  .markdown-body :global(a:hover) {
    border-bottom-color: var(--accent);
  }

  .markdown-body :global(table) {
    border-collapse: collapse;
    margin: 0.6rem 0;
    width: 100%;
    font-size: 0.85rem;
  }

  .markdown-body :global(th),
  .markdown-body :global(td) {
    border: 1px solid var(--border);
    padding: 0.45rem 0.7rem;
    text-align: left;
  }

  .markdown-body :global(th) {
    background: var(--bg-tertiary);
    font-weight: 600;
    color: var(--text);
  }

  .markdown-body :global(tr:nth-child(even) td) {
    background: var(--bg-secondary);
  }

  .markdown-body :global(hr) {
    border: none;
    border-top: 1px solid var(--border);
    margin: 1rem 0;
  }

  .markdown-body :global(strong) {
    font-weight: 600;
    color: var(--text);
  }

  .markdown-body :global(em) {
    font-style: italic;
  }

  .markdown-body :global(del) {
    color: var(--text-faint);
  }

  .markdown-body :global(img) {
    max-width: 100%;
    border-radius: var(--radius-sm);
  }
</style>
