# NAVI Goal System (Sistema de Goals)

## Visão geral

O sistema de goals do NAVI permite associar um objetivo persistente a uma sessão
de agente. O goal guia o agente através de múltiplos turns, com tracking de
budget/tokens, auto-continuação, e steering via injeção de prompts.

## Estrutura de dados

```rust
/// crate::goal::types
pub struct SessionGoal {
    pub session_id: String,
    pub goal_id: GoalId,            // ID único baseado em timestamp
    pub objective: String,          // O objetivo textual
    pub status: GoalStatus,         // Máquina de estados
    pub token_budget: Option<i64>,  // Budget opcional de tokens
    pub tokens_used: i64,           // Tokens consumidos
    pub time_used_seconds: i64,     // Tempo decorrido
    pub consecutive_blocked_turns: u32,
    pub block_reason: Option<String>,
    pub created_at: u64,
    pub updated_at: u64,
}
```

## Máquina de estados (GoalStatus)

```
Active ───────────────────────────────► Complete
  │                                       ▲
  ├──► Paused (pelo usuário)              │
  ├──► Blocked (3+ turns consecutivos     │
  │     com mesmo bloqueio, ou erro fatal) │
  ├──► UsageLimited (limite de uso        │
  │     da API atingido)                  │
  └──► BudgetLimited (token_budget        │
        excedido) ────────────────────────┘
```

Estados terminais: `Complete`, `BudgetLimited`.

## Componentes principais

### 1. GoalService — API pública (`goal/service.rs`)

- `set_goal(session_id, objective, token_budget)` — cria ou atualiza um goal
- `get_goal(session_id)` — lê o goal atual
- `clear_goal(session_id)` — remove o goal
- `update_goal_status(session_id, status)` — atualiza status
- `register_runtime(session_id, runtime)` / `unregister_runtime(session_id)` — gerencia runtimes
- `persist_goal(session_id, project_dir, session_store)` — persiste o goal em disco
- `load_goal(session_id, session_store)` — carrega goal persistido

Mantém um registro `HashMap<String, Arc<GoalRuntimeHandle>>` de todos os runtimes ativos.

### 2. GoalRuntimeHandle — Runtime por sessão (`goal/runtime.rs`)

Gerencia o ciclo de vida do goal em uma sessão viva:

- `continue_if_idle()`: Quando a thread fica idle e o goal está Active, retorna um
  prompt de continuação para ser injetado pelo sistema
- `record_blocked_turn(reason)`: Registra um turn bloqueado. Após 3+ turns consecutivos
  com o mesmo bloqueio, transiciona para `Blocked`
- `mark_usage_limited()`: Transiciona para `UsageLimited` em limite de API
- `set_objective(objective, token_budget)`: Atualiza o objetivo do goal
- `set_auto_continue(enabled)`: Habilita/desabilita auto-continuação
- `start_turn()` / `finish_turn()` / `record_tokens(delta)`: Accounting por turn

### 3. GoalAccountingState — Contabilidade por turn (`goal/accounting.rs`)

- Trackeia tokens e tempo consumidos por goal ativo
- `start_turn()` — começa a contar tokens/tempo para um turn
- `record_token_usage(delta)` — registra delta de tokens
- `finish_turn()` — finaliza turn, grava tempo decorrido, retorna snapshot
- `snapshot()` — tira snapshot do goal atual
- Usa `Mutex` interno para serializar escritas de progresso

### 4. Tools expostas ao modelo (`goal/tools.rs`)

Três tools que o modelo pode chamar:

- **get_goal** — Lê o goal atual (objective, status, tokens usados, budget restante,
  tempo decorrido, contagem de bloqueios)
- **create_goal** — Cria um goal novo (só quando explicitamente solicitado pelo usuário)
- **update_goal** — Atualiza status do goal:
  - `complete`: quando objetivo foi atingido e verificado
  - `blocked`: quando o mesmo bloqueio ocorreu por 3+ turns consecutivos
  - `pause` / `resume`: controle manual de auto-continuação

Cada tool é uma struct separada (`GetGoalTool`, `CreateGoalTool`, `UpdateGoalTool`)
que implementa o trait `Tool`, permitindo registro individual no `ToolExecutor`.

### 5. GoalExtension — Hooks de integração (`goal/extension.rs`)

Fornece hooks explícitos para o ciclo de vida de sessão e turn:

| Hook | Ação |
|---|---|
| `on_session_start(session_id)` | Registra o runtime no GoalService |
| `on_session_resume(session_id)` | Placeholder para restaurar goal após resume |
| `on_session_end(session_id)` | Remove o runtime e limpa o goal |
| `on_idle()` | Chama `continue_if_idle()` → retorna prompt de continuação |
| `on_turn_start(session_id, task)` | Inicia accounting de tokens/tempo |
| `on_turn_end(session_id)` | Finaliza accounting do turn |
| `on_turn_abort(session_id)` | Accounting + cleanup |
| `on_turn_error(error_message)` | Se Usage Limit → `UsageLimited`; erros fatais → `Blocked` |
| `on_tool_complete()` | Hook para pós-tool (accounting conduzido por TokensUpdated) |
| `on_token_usage(input, output)` | Registra uso de tokens durante o turn; checa budget |

### 6. Steering — Injeção de prompts no contexto (`goal/steering.rs`)

Três templates (inline, sem arquivos externos):

- **continuation.md** (função `build_continuation_prompt`): Injetado quando a thread
  fica idle com goal ativo. Contém:
  - O objetivo como `<objective>...</objective>`
  - Status, tokens usados/budget, tempo decorrido
  - Instruções de comportamento: "Continue working toward the active thread goal"
  - Regras de completion audit (verificação rigorosa antes de marcar complete)
  - Regras de blocked audit (3+ turns consecutivos com mesmo bloqueio)

- **budget_limit.md** (função `build_budget_limit_prompt`): Injetado quando
  `token_budget` é excedido. Avisa o modelo que o budget acabou e ele deve finalizar.

- **objective_updated.md** (função `build_objective_updated_prompt`): Injetado quando
  o usuário muda o objetivo de um goal ativo.

Os prompts são injetados como contexto interno do sistema (prefixados com
`# Active Thread Goal`, `# Budget Limit Reached`, ou `# Objective Updated`).

## Fluxo completo

```
1. Usuário/cliente define um goal via GoalService.set_goal()
         │
2. GoalService cria ou atualiza o goal no GoalRuntimeHandle
         │
3. Goal é registrado no runtime da sessão
         │
4. No próximo turn:
   ├─ on_turn_start → accounting começa
   ├─ Modelo vê as tools: get_goal, create_goal, update_goal
   ├─ A cada token usage: on_token_usage → update do delta + check budget
   └─ on_turn_end → accounting final + persist
         │
5. Goal é persistido via persist_goal() junto com session snapshot
         │
6. Se thread fica idle e goal está Active:
   └─ on_idle → continue_if_idle()
      └─ Retorna prompt de continuation
      └─ O caller externo (TUI/headless) injeta o prompt e dispara novo turn
         │
7. Durante continuação:
   ├─ Se token_budget for excedido → BudgetLimited
   │  └─ Steering: budget_limit.md injetado
   ├─ Se modelo chama update_goal(complete) → Complete
   ├─ Se erro fatal → Blocked
   └─ Se API usage limit → UsageLimited
         │
8. Goal ativo persiste entre sessões (carregado via load_goal)
```

## API (navi-core)

```rust
use navi_core::goal::{
    GoalService, GoalRuntimeHandle, GoalExtension,
    SessionGoal, GoalStatus, GoalId,
    GetGoalTool, CreateGoalTool, UpdateGoalTool,
};

// Criar serviço
let service = Arc::new(GoalService::new());

// Criar runtime handle para uma sessão
let runtime = Arc::new(GoalRuntimeHandle::new(None));
service.register_runtime(session_id.clone(), runtime.clone());

// Criar extensão para hooks
let extension = GoalExtension::new(service.clone(), runtime.clone());

// Definir um goal
let goal = service.set_goal(
    "session-123".to_string(),
    "Implementar feature X".to_string(),
    Some(100_000), // token budget
);

// Hooks de turn
extension.on_session_start("session-123");
extension.on_turn_start("session-123", "implement feature X");
extension.on_token_usage(5000, 2000);
extension.on_turn_end("session-123");

// Verificar auto-continuação
if let Some(prompt) = extension.on_idle() {
    // Injeta prompt no contexto e dispara novo turn
}
```

## Persistência

Goals são persistidos como `goal.json` no diretório da sessão (`<data_dir>/sessions/<session_id>/goal.json`).
O formato é JSON com todos os campos do `SessionGoal`.

```json
{
  "session_id": "session-123",
  "goal_id": "goal-1700000000-0001",
  "objective": "Implementar feature X",
  "status": "active",
  "token_budget": 100000,
  "tokens_used": 15234,
  "time_used_seconds": 120,
  "consecutive_blocked_turns": 0,
  "block_reason": null,
  "created_at": 1700000000,
  "updated_at": 1700000120
}
```

Para carregar: `service.load_goal(session_id, &session_store)`
Para salvar: `service.persist_goal(session_id, &project_dir, &session_store)`

## Integração com TUI

O TUI deve:

1. Criar `GoalService` e `GoalRuntimeHandle` ao iniciar uma sessão
2. Registrar as 3 goal tools no `ToolExecutor` via `register_tool()`
3. Chamar os hooks nos pontos apropriados do ciclo de vida
4. Após cada turn, verificar `extension.on_idle()` para auto-continuação
5. Se retornar um prompt, injetá-lo como contexto interno e disparar novo turn
6. Persistir o goal junto com session snapshots

## Testes

Para testar o sistema de goals:

```bash
just test-crate navi-core
# Testes específicos (a adicionar):
# cargo test -p navi-core -- goal
```
