import type { ToolCallInfo } from "./types";

export type ToolTone = "inspect" | "change" | "command" | "browser" | "neutral";

export interface ToolPresentation {
  verb: string;
  target: string;
  detail?: string;
  tone: ToolTone;
  expandable: boolean;
}

function record(value: unknown): Record<string, unknown> {
  return value && typeof value === "object" && !Array.isArray(value)
    ? (value as Record<string, unknown>)
    : {};
}

function stringField(input: Record<string, unknown>, keys: string[]): string | undefined {
  for (const key of keys) {
    const value = input[key];
    if (typeof value === "string" && value.trim()) return value.trim();
  }
  return undefined;
}

function truncate(value: string, max = 80): string {
  const compact = value.replace(/\s+/g, " ").trim();
  return compact.length > max ? `${compact.slice(0, max - 1)}…` : compact;
}

function outputDetail(output: unknown): string | undefined {
  const result = record(output);
  const values = [result.results, result.matches, result.hits, result.files];
  const collection = values.find(Array.isArray);
  if (collection) return `${collection.length} resultado${collection.length === 1 ? "" : "s"}`;
  if (typeof result.count === "number") return `${result.count} resultado${result.count === 1 ? "" : "s"}`;
  return undefined;
}

export function formatToolValue(value: unknown, max = 5000): string {
  if (value === undefined || value === null) return "";
  if (typeof value === "string") return value.slice(0, max);
  try {
    return JSON.stringify(value, null, 2).slice(0, max);
  } catch {
    return String(value).slice(0, max);
  }
}

export function presentationFor(call: ToolCallInfo): ToolPresentation {
  const name = call.name.toLowerCase();
  const input = record(call.input);
  const path = stringField(input, [
    "path",
    "file",
    "file_path",
    "filePath",
    "relative_path",
    "relativePath",
    "target",
  ]);
  const query = stringField(input, ["query", "pattern", "substring_pattern", "regex"]);
  const command = stringField(input, ["command", "cmd"]);
  const description = stringField(input, ["description", "summary"]);
  const action = stringField(input, ["action", "operation", "op", "method", "type"]);
  const detail = outputDetail(call.output);

  if (name.includes("browser") || name.includes("chrome") || name.includes("web")) {
    return {
      verb: "Usando navegador",
      target: truncate(stringField(input, ["url", "href", "text"]) ?? action ?? "página"),
      tone: "browser",
      expandable: true,
    };
  }

  if (name === "search" || name === "grep" || name.includes("search")) {
    return {
      verb: "Buscando",
      target: truncate(query ?? path ?? "no projeto"),
      detail,
      tone: "inspect",
      expandable: Boolean(call.input || call.output),
    };
  }

  if (name === "read" || name.includes("read_file") || name === "view_file" || name === "cat") {
    return {
      verb: "Lendo",
      target: truncate(path ?? "arquivo"),
      detail,
      tone: "inspect",
      expandable: Boolean(call.input || call.output),
    };
  }

  if (name === "list_dir" || name === "list_files" || name === "glob" || name === "fs_browser") {
    return {
      verb: "Listando",
      target: truncate(path ?? query ?? "arquivos"),
      detail,
      tone: "inspect",
      expandable: Boolean(call.input || call.output),
    };
  }

  if (name === "edit" || name.includes("edit_file") || name.includes("search_replace") || name.includes("apply_patch")) {
    return {
      verb: "Editando",
      target: truncate(path ?? "arquivos do projeto"),
      detail,
      tone: "change",
      expandable: true,
    };
  }

  if (name === "write" || name.includes("write_file") || name.includes("create_file")) {
    return {
      verb: "Escrevendo",
      target: truncate(path ?? "arquivo"),
      detail,
      tone: "change",
      expandable: true,
    };
  }

  if (name === "run" || name === "bash" || name.includes("test") || name.includes("build") || name.includes("git")) {
    return {
      verb: name.includes("test") ? "Rodando testes" : name.includes("build") ? "Compilando" : "Executando",
      target: truncate(description ?? command ?? action ?? "comando"),
      detail,
      tone: "command",
      expandable: true,
    };
  }

  if (name === "set_session_title") {
    return {
      verb: "Atualizando título",
      target: truncate(stringField(input, ["title", "name"]) ?? "conversa"),
      tone: "neutral",
      expandable: false,
    };
  }

  if (name === "tool_search") {
    return {
      verb: "Procurando ferramenta",
      target: truncate(query ?? "ferramentas disponíveis"),
      tone: "inspect",
      expandable: false,
    };
  }

  return {
    verb: "Executando",
    target: truncate(description ?? path ?? command ?? call.name),
    detail,
    tone: "neutral",
    expandable: Boolean(call.input || call.output),
  };
}
