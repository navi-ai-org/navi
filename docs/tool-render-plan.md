# Plano de Renderização Rica para Tools da TUI

## Problema

33 das ~39 tools registradas no `ToolExecutor` mostram apenas informação genérica na TUI:

- **Linha compacta**: só o nome da tool humanizado (ex: `Process`, `Plan`, `Code`)
- **View expandida** (Ctrl+O / full tool view): `"<tool_name> completed successfully"`

Apenas 6 tools têm renderização específica: `read_file`, `write_file`, `apply_patch`, `bash`, `grep`, `fs_browser`.

O código está em `crates/navi-tui/src/render/tool.rs` — funções `tool_compact_text` (linha 20) e `formatted_tool_output`/`generic_tool_summary`.

---

## Estrutura do Código Atual

```rust
// tool_compact_text — linha compacta (sempre visível)
fn tool_compact_text(invocation, result) -> String {
    match invocation.tool_name.as_str() {
        "read_file" | "view_file" => read_file_summary(...),
        "write_file"              => write_file_summary(...),
        "apply_patch"             => apply_patch_summary(...),
        "bash"                    => bash_summary(...),
        "grep"                    => grep_summary(...),
        "fs_browser"              => fs_browser_summary(...),
        name                      => humanize_tool_name(name),  // GENÉRICO
    }
}

// tool_full_content — view expandida (Ctrl+O)
fn tool_full_content(invocation, result) -> String {
    let mut content = format!("✓/✗ {}\n\n", tool_compact_text(...));
    if let Some(formatted) = formatted_tool_output(invocation, result) {
        content.push_str(&formatted);    // ESPECÍFICO
    } else {
        content.push_str(&generic_tool_summary(invocation, result));  // GENÉRICO
    }
}

// generic_tool_summary — fallback genérico
fn generic_tool_summary(invocation, result) -> String {
    if result.ok {
        format!("{} completed successfully\n", invocation.tool_name)
    } else if let Some(error) = ... {
        format!("Error: {error}\n")
    } else {
        format!("{} failed\n", invocation.tool_name)
    }
}
```

A chave é `formatted_tool_output`: se retornar `Some`, é renderização rica; se `None`, cai no `generic_tool_summary`. O `match` interno do `formatted_tool_output` cobre apenas `read_file`, `write_file`, `apply_patch`, `bash`, `grep`, `fs_browser` e trata erros.

---

## Plano por Tool

Para cada tool listamos:
- **Output fields**: campos que a tool retorna no `result.output` JSON
- **Linha compacta**: o que mostrar na view default (1 linha)
- **View expandida**: o que mostrar com Ctrl+O

### Grupo 1: Process & Command Tools

#### 1. `process`
- **Output fields**: `process_id`, `status` ("running"/"exited"/"killed"), `exit_code`, `stdout`, `stderr`, `elapsed_ms`, `background`
- **Linha compacta**: `Run <command> (exit N)` ou `Run <command> (running · Xs)` se background
- **View expandida**: Mostrar command, status, exit code, elapsed; se tiver stdout/stderr, mostrar em blocos ``` (truncar a 5000 linhas, igual ao bash)

#### 2. `test_runner`
- **Output fields**: `framework`, `passed`, `failed`, `skipped`, `duration_ms`, `failures[]` (cada `{name, message, location?}`), `summary`, `raw_output`
- **Linha compacta**: `Test (passed N, failed M, skipped K · X.Xs)`
- **View expandida**: Framework detectado, sumário de pass/fail/skip, lista de failures com nome e mensagem, duração. Se raw_output, mostrar em bloco ``` truncado.

#### 3. `build_runner`
- **Output fields**: `success`, `incremental` (bool), `warnings[]`, `errors[]`, `duration_ms`, `raw_output?`
- **Linha compacta**: `Build (success/failed, N warnings, M errors · X.Xs)` ou `Build (cached, up to date)`
- **View expandida**: Status, incremental/cached info, lista de warnings (path:line: message), lista de errors (path:line: message), duração.

---

### Grupo 2: Code Intelligence Tools

#### 4. `code` (CodeReadTool — aliases: `code`)
- **Output fields** (varia por action):
  - `overview`: `symbols[]` (cada `{name, kind, line, path, signature?}`), `files_scanned`, `truncated`, `path`
  - `find`: `symbols[]` (cada `{name, kind, line, path, signature?, hash?}`), `truncated`, `query`
  - `references`: `references[]` (cada `{name, kind, line, path}`), `truncated`, `query`
  - `diagnostics`: `diagnostics[]` (cada `{path, line, message, severity}`), `files_scanned`, `path`
- **Linha compacta**:
  - overview: `Code overview <path> (N symbols)`
  - find: `Find "<query>" (N results)`
  - references: `References to "<query>" (N refs)`
  - diagnostics: `Diagnostics <path> (N issues)`
- **View expandida**: Tabela/listagem dos resultados com path:line:name:signature

#### 5. `code_edit`
- **Output fields**: `action`, `path`, `edits` (número de edições), `start_line`, `end_line`
- **Linha compacta**: `Code edit <path> (N edits, lines S-E)`
- **View expandida**: Action type, path, número de edições, range de linhas afetado. Explicitar se foi replace/insert-before/insert-after/rename.

#### 6. `code_exec`
- **Output fields**: `status`, `stdout`, `stderr`, `exit_code`, `elapsed_ms`
- **Linha compacta**: `Code exec (exit N · X.Xs)`
- **View expandida**: Command executado, exit code, stdout/stderr em blocos ``` truncados.

#### 7. `ast_search`
- **Output fields**: `matches[]` (cada `{name, kind, line, path, signature?}`), `truncated`, `query`
- **Linha compacta**: `AST search "<query>" (N matches)`
- **View expandida**: Listagem de matches com path:line:kind:name:signature. Bloco ``` com resumo.

#### 8. `symbol_goto`
- **Output fields**: `name`, `path`, `line`, `kind`?
- **Linha compacta**: `Goto <name> → <path>:<line>`
- **View expandida**: Symbol name, kind, path, line number.

#### 9. `symbol_references`
- **Output fields**: `references[]` (cada `{name, kind, line, path}`), `truncated`, `query`
- **Linha compacta**: `References to "<query>" (N refs)`
- **View expandida**: Listagem de refs com path:line:name.

#### 10. `dependency_graph_query`
- **Output fields**: `edges[]` (cada `{from, to, kind?}`), `truncated`
- **Linha compacta**: `Dependency graph (N edges)`
- **View expandida**: Tabela de edges: from → to.

#### 11. `test_discovery`
- **Output fields**: `command` (sugestão de comando de teste), `paths[]`
- **Linha compacta**: `Test discovery → <command>`
- **View expandida**: Comando sugerido, paths afetados listados.

#### 12. `ownership_churn_query`
- **Output fields**: `files[]` (cada `{path, churn, ...}`)
- **Linha compacta**: `Churn query (N files)`
- **View expandida**: Tabela de arquivos por churn: path | churn score.

---

### Grupo 3: Repo Intelligence & Repo Explore

#### 13. `repo_explore`
- **Output fields**: `locations` (texto markdown do subagent), `elapsed_ms`
- **Linha compacta**: `Repo explore "<query>" (X.Xs)`
- **View expandida**: Renderizar o campo `locations` como markdown (passar pelo `render_markdown_lines`). Mostrar elapsed time.

#### 14. `search` / `list_dir` / `glob` (SearchTool com action-based dispatch)
- **Output fields** (variam, o SearchTool tem actions: `grep`, `list`, `tree`, `find`, `stat`):
  - `grep`: `matches[]`, `truncated`, `pattern`, `path` — **já coberto pelo tool `grep`**
  - `list`: `files[]` ou `entries[]`, `path`, `count`
  - `tree`: `entries[]` (árvore aninhada), `path`
  - `find`: `files[]`, `pattern`, `count`
  - `stat`: `path`, `size`, `modified`, `is_dir`, etc.
- **Linha compacta**:
  - list: `List <path> (N items)`
  - tree: `Tree <path> (N items)`
  - find: `Find "<pattern>" (N files)`
  - stat: `Stat <path> (N bytes)`
- **View expandida**: Listagem de arquivos/diretórios (com path, size, type), tree renderizado com indentação.

**Nota**: Estas tools (`search`, `list_dir`, `glob`) são registradas como aliases do `SearchTool` com diferentes actions default. É preciso verificar como o `tool_name` chega no `ToolInvocation` — pode ser `search`, `list_dir`, ou `glob` mesmo que internamente usem o mesmo código.

---

### Grupo 4: Planning & Session Tools

#### 15. `plan`
- **Output fields** (varia por action):
  - `create`: `plan` (objeto `{id, title, description, steps[], status, created_at}`)
  - `update`: `plan` (objeto atualizado) ou `error`
  - `complete_step`: `plan` (objeto atualizado), `step_completed` (índice)
  - `get`: `plan` (objeto completo)
  - `list`: `plans[]` (array de planos), `count`
  - `active`: `plan` (objeto)
- **Linha compacta**: `Plan <action> "<title>"` ou `Plan <action> (N steps)`
- **View expandida**: Mostrar o plano: título, descrição, steps com checkboxes [x]/[ ] e notes. Status e timestamps.

#### 16. `init_session`
- **Output fields**: `goal`, `features[]` (cada `{id, title, description, verification_steps[]}`), `session_file`
- **Linha compacta**: `Init session "<goal>" (N features)`
- **View expandida**: Goal, lista de features com id, título e verification steps.

#### 17. `mark_feature_done`
- **Output fields**: `feature_id`, `passed`, `verification_results[]` (cada `{step, ok, output?}`)
- **Linha compacta**: `Mark done <feature_id> (N/N checks passed)`
- **View expandida**: Feature ID, cada step de verificação com ✓/✗ e output relevante.

---

### Grupo 5: Interaction Tools

#### 18. `question`
- **Output fields**: (tool retorna erro se chamada sem cliente interativo) `error`
- **Linha compacta**: `Question "<title>"` ou `Question (interactive only)`
- **View expandida**: Mostrar a pergunta que foi feita e as opções, se disponíveis.

#### 19. `request_user_input`
- **Output fields**: (similar ao question, erro em headless) `error`
- **Linha compacta**: `Request input "<title>"` ou `Request input (interactive only)`
- **View expandida**: Mostrar título e descrição do input solicitado.

#### 20. `append_note`
- **Output fields**: `path` (caminho do notes.md), `appended` (bool), `bytes_written`
- **Linha compacta**: `Append note (N bytes)`
- **View expandida**: Path do notes.md, bytes escritos, preview do conteúdo (primeiras linhas).

---

### Grupo 6: Utility Tools

#### 21. `current_time`
- **Output fields**: `utc_iso`, `unix_timestamp_seconds`, `timezone`
- **Linha compacta**: `Current time: <utc_iso>`
- **View expandida**: UTC ISO, Unix timestamp, timezone.

#### 22. `sleep`
- **Output fields**: `slept_seconds`
- **Linha compacta**: `Sleep (Xs)`
- **View expandida**: `Slept for X seconds`.

#### 23. `get_context_remaining`
- **Output fields**: `context_window`, `used_tokens`, `remaining_tokens`, `usage_percent`, `status`
- **Linha compacta**: `Context: <remaining> / <window> (<usage_percent>)`
- **View expandida**: Status message, token breakdown: window / used / remaining / percentage.

#### 24. `view_image` / `inspect_image`
- **Output fields**: `path`, `format`, `size_bytes`, `hint`
- **Linha compacta**: `View image <path> (<format>, N bytes)`
- **View expandida**: Path, format, size (humanizado: KB/MB), hint message.

#### 25. `new_context_window`
- **Output fields**: `new_context_requested` (bool), `summary`, `message`
- **Linha compacta**: `New context window requested`
- **View expandida**: Message e preview do summary (truncado).

#### 26. `tool_search`
- **Output fields**: `results[]` (cada `{name, description, kind, input_schema?, tags[]}`), `query`, `count`
- **Linha compacta**: `Tool search "<query>" (N results)`
- **View expandida**: Tabela de results: name | kind | description (truncada).

#### 27. `verifier`
- **Output fields**: `command` (executado), `exit_code`, `stdout`, `stderr`, `passed` (bool), `duration_ms`
- **Linha compacta**: `Verify (passed/failed · exit N · X.Xs)`
- **View expandida**: Command, exit code, duration, stdout/stderr em blocos ```. Pass/fail em destaque.

#### 28. `runtime_info`
- **Output fields**: `harness_profile`, `project_root`, `config_path`?, `model`?, `session_id`?
- **Linha compacta**: `Runtime info: <harness_profile> profile`
- **View expandida**: Tabela com project root, config path, model, session ID, harness profile.

#### 29. `branch_race`
- **Output fields**: (provavelmente `branch`, `behind`, `ahead`, `status`)
- **Linha compacta**: `Branch race: <branch>` ou `Branch race (behind N, ahead M)`
- **View expandida**: Branch name, ahead/behind counts, status message.

---

### Grupo 7: Subagent & Background

#### 30. `subagent`
- **Output fields**: (background/non-background): `invocation_id`? `task_id`? `result`? (texto da resposta)
- **Linha compacta**: `Subagent "<prompt truncado>" (running/completed)`
- **View expandida**: Prompt, result text renderizado como markdown, elapsed time.
- **Nota**: Já existe renderização especial para subagent em `render_compact_tool_result` (ChatLineSource::Subagent). A linha compacta atual é `humanize_tool_name("subagent")` → `Subagent`. O tratamento já é diferente do genérico no `render_compact_tool_result`.

#### 31. `history_ops`
- **Output fields**: (varia por action: `search`, `recent`, `get`, `summaries`):
  - `search`: `events[]`, `count`, `query`
  - `recent`: `events[]`, `count`, `session_id`
  - `get`: `event` (objeto)
  - `summaries`: `summaries[]`, `count`
- **Linha compacta**: `History <action> (N results)`
- **View expandida**: Listagem dos eventos/summaries encontrados, truncados.

#### 32. `sandbox`
- **Output fields**: (tool retorna erro em headless) `error`, `changes_summary` (se bem-sucedido)
- **Linha compacta**: `Sandbox (N changes)` ou `Sandbox (interactive only)`
- **View expandida**: ChangeSetSummary: arquivos modificados, linhas adicionadas/removidas.

#### 33. `package_manager`
- **Output fields**: `action`, `packages[]`, `manager` (detectado), `stdout`, `stderr`, `exit_code`, `ok`
- **Linha compacta**: `Package <action> <pkg1, pkg2...> (<manager>)` ou `Package <action> (N packages)`
- **View expandida**: Manager detectado, action, lista de packages, stdout/stderr em blocos ``` truncados.

---

## Plano de Implementação

### Fase 1: Adicionar funções de summary para cada tool (compacto)

Em `crates/navi-tui/src/render/tool.rs`, expandir o `match` do `tool_compact_text`:

```rust
pub(crate) fn tool_compact_text(invocation: &ToolInvocation, result: &ToolResult) -> String {
    let mut text = match invocation.tool_name.as_str() {
        // Existentes (manter):
        "read_file" | "view_file" => read_file_summary(invocation, result),
        "write_file" => write_file_summary(invocation, result),
        "apply_patch" => apply_patch_summary(invocation),
        "bash" => bash_summary(invocation, result),
        "grep" => grep_summary(invocation, result),
        "fs_browser" => fs_browser_summary(invocation, result),

        // NOVOS:
        "process" => process_summary(invocation, result),
        "test_runner" => test_runner_summary(result),
        "build_runner" => build_runner_summary(result),
        "code" => code_summary(invocation, result),
        "code_edit" => code_edit_summary(result),
        "code_exec" => code_exec_summary(result),
        "ast_search" => ast_search_summary(invocation, result),
        "symbol_goto" => symbol_goto_summary(result),
        "symbol_references" => symbol_references_summary(invocation, result),
        "dependency_graph_query" => dep_graph_summary(result),
        "test_discovery" => test_discovery_summary(result),
        "ownership_churn_query" => churn_summary(result),
        "repo_explore" => repo_explore_summary(invocation, result),
        "search" | "list_dir" | "glob" => search_tool_summary(invocation, result),
        "plan" => plan_summary(invocation, result),
        "init_session" => init_session_summary(result),
        "mark_feature_done" => mark_feature_done_summary(result),
        "question" => question_summary(invocation),
        "request_user_input" => request_user_input_summary(invocation),
        "append_note" => append_note_summary(result),
        "current_time" => current_time_summary(result),
        "sleep" => sleep_summary(result),
        "get_context_remaining" => context_remaining_summary(result),
        "view_image" | "inspect_image" => view_image_summary(result),
        "new_context_window" => new_context_window_summary(result),
        "tool_search" => tool_search_summary(invocation, result),
        "verifier" => verifier_summary(result),
        "runtime_info" => runtime_info_summary(result),
        "branch_race" => branch_race_summary(result),
        "subagent" => subagent_compact_summary(invocation, result),
        "history_ops" => history_ops_summary(invocation, result),
        "sandbox" => sandbox_summary(result),
        "package_manager" => package_manager_summary(invocation, result),

        name => humanize_tool_name(name),
    };
    // ... error handling existente
}
```

### Fase 2: Expandir `formatted_tool_output` com renderização rica

Adicionar branches no `match` interno de `formatted_tool_output` para cada tool. Cada branch retorna `Some(content)` com markdown formatado.

### Fase 3: Testes

Adicionar testes unitários em `crates/navi-tui/src/render.rs` (módulo `tests`) para cada nova função de summary.

### Fase 4: Verificação

- `just test-crate navi-tui`
- Testar visualmente no TUI com cada tool

---

## Priorização

**Prioridade alta** (tools mais usadas, maior impacto visual):
1. `code`, `code_edit`, `code_exec` — usadas em todo turn
2. `plan`, `init_session`, `mark_feature_done` — usadas em tarefas longas
3. `test_runner`, `build_runner` — usadas em validação
4. `package_manager` — usada em instalação de deps
5. `search`, `list_dir`, `glob` — usadas como aliases de search
6. `ast_search`, `symbol_goto`, `symbol_references` — code intelligence
7. `process` — execução de comandos
8. `subagent` — subagentes

**Prioridade média** (tools utilitárias):
9. `current_time`, `sleep`, `get_context_remaining`
10. `append_note`, `new_context_window`
11. `question`, `request_user_input`
12. `repo_explore`, `tool_search`, `history_ops`
13. `verifier`, `dependency_graph_query`, `test_discovery`, `ownership_churn_query`

**Prioridade baixa** (tools raramente usadas ou de configuração):
14. `view_image`, `inspect_image`
15. `sandbox`, `branch_race`, `runtime_info`
