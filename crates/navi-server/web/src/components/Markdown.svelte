<script lang="ts">
  import { marked } from "marked";

  let { content }: { content: string } = $props();

  // Configure marked for GitHub-flavored markdown
  marked.setOptions({
    gfm: true,
    breaks: true,
  });

  // Render to HTML. marked is synchronous by default when async option is false.
  let html = $derived(marked.parse(content, { async: false }) as string);
</script>

<!-- eslint-disable svelte/no-vhtml -->
<div class="markdown-body">{@html html}</div>

<style>
  .markdown-body {
    font-size: 0.9rem;
    line-height: 1.6;
    word-wrap: break-word;
    overflow-wrap: break-word;
  }

  .markdown-body :global(p) {
    margin: 0.4rem 0;
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
    margin: 0.8rem 0 0.4rem;
    font-weight: 600;
    line-height: 1.3;
  }

  .markdown-body :global(h1) {
    font-size: 1.3rem;
  }

  .markdown-body :global(h2) {
    font-size: 1.15rem;
  }

  .markdown-body :global(h3) {
    font-size: 1.05rem;
  }

  .markdown-body :global(ul),
  .markdown-body :global(ol) {
    margin: 0.4rem 0;
    padding-left: 1.5rem;
  }

  .markdown-body :global(li) {
    margin: 0.2rem 0;
  }

  .markdown-body :global(code) {
    font-family: var(--font-mono);
    font-size: 0.85em;
    background: var(--bg-tertiary);
    padding: 0.15rem 0.35rem;
    border-radius: 3px;
  }

  .markdown-body :global(pre) {
    background: var(--bg);
    border: 1px solid var(--border);
    border-radius: var(--radius-sm);
    padding: 0.75rem;
    overflow-x: auto;
    margin: 0.5rem 0;
  }

  .markdown-body :global(pre code) {
    background: transparent;
    padding: 0;
    font-size: 0.85rem;
    line-height: 1.5;
  }

  .markdown-body :global(blockquote) {
    border-left: 3px solid var(--accent);
    margin: 0.5rem 0;
    padding: 0.3rem 0.8rem;
    color: var(--text-muted);
  }

  .markdown-body :global(a) {
    color: var(--accent);
    text-decoration: none;
  }

  .markdown-body :global(a:hover) {
    text-decoration: underline;
  }

  .markdown-body :global(table) {
    border-collapse: collapse;
    margin: 0.5rem 0;
    width: 100%;
  }

  .markdown-body :global(th),
  .markdown-body :global(td) {
    border: 1px solid var(--border);
    padding: 0.4rem 0.6rem;
    text-align: left;
  }

  .markdown-body :global(th) {
    background: var(--bg-tertiary);
    font-weight: 600;
  }

  .markdown-body :global(hr) {
    border: none;
    border-top: 1px solid var(--border);
    margin: 0.8rem 0;
  }

  .markdown-body :global(strong) {
    font-weight: 600;
  }

  .markdown-body :global(em) {
    font-style: italic;
  }
</style>
