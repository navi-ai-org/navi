# Navi Harness Beyond Parity Plan

Data: 2026-06-27

Este documento define o plano para o Navi ultrapassar os harnesses modernos, depois de atingir a paridade descrita em `docs/harness-parity-plan.md`.

## Objetivo final

Transformar o Navi em um agent operating system verificavel que aprende com cada execucao:

- Verifier-first loop como default.
- Repo intelligence baseada em AST/LSP/code graph.
- Code mode com SDK tipado e nested tools controladas.
- Branch racing com worktrees/snapshots e escolha por evidencia.
- Subagentes especializados com perfis, toolsets, budgets e reviewer independente.
- Permission engine por efeito real e capability ledger.
- MCP firewall com allowlist, provenance, taint tracking e auditoria.
- Memoria operacional em camadas.
- Auto-skill mining a partir de traces bem-sucedidos.
- Trace-to-eval e replay de regressao.
- Multi-model routing orientado por metricas.
- Metrica principal: `verified_success_per_1k_tokens`.

## Pre-requisito

Nao iniciar este plano antes de cumprir o gate de paridade:

- Tool metadata v2 existe.
- `tool.search` existe.
- Snapshot/rollback MVP existe.
- Effect report v1 existe.
- Verifier API existe.
- Trace store existe.
- MCP tools passam por registry/policy/trace.
- Subagentes tem escopo e approvals controlados.

## Principio de arquitetura

O Navi deve ganhar por harness intelligence, nao por acoplar produto ou UI:

- O TUI continua cliente.
- `navi-core` concentra runtime, tools, safety, memory, verifier, trace e orchestration.
- `navi-sdk` expoe APIs serializaveis para TUI, ACP, Tutor e clientes locais.
- Superficies como IDE/CI/web/cloud sao adapters posteriores, nao o core deste plano.

## Fontes locais obrigatorias para comparacao continua

Use os mesmos repositorios locais do plano de paridade e aprofunde por area:

### Codex local

- Code mode: `/home/enrell/lab/codex/codex-rs/code-mode/src/`
- Tool routing: `/home/enrell/lab/codex/codex-rs/core/src/tools/`
- Sandbox/exec: `/home/enrell/lab/codex/codex-rs/core/src/unified_exec/`
- Tool trace: `/home/enrell/lab/codex/codex-rs/core/src/tools/tool_dispatch_trace.rs`
- Multi-agent/tool exposure tests: `/home/enrell/lab/codex/codex-rs/core/src/tools/spec_plan_tests.rs`

### Claude Code local

- Tool model: `/home/enrell/lab/claude-code/src/Tool.ts`
- Tool catalog: `/home/enrell/lab/claude-code/docs/tools.md`
- Permissions: `/home/enrell/lab/claude-code/src/hooks/toolPermission/`
- MCP: `/home/enrell/lab/claude-code/src/services/mcp/`
- Memory: `/home/enrell/lab/claude-code/src/memdir/`
- Subsystems: `/home/enrell/lab/claude-code/docs/subsystems.md`

## Roadmap resumido

| Sprint | Tema | Resultado esperado |
|---|---|---|
| B0 | Superiority baseline | Evals e metricas para provar vantagem |
| B1 | AST/LSP/code graph | Menos grep/read, mais precisao |
| B2 | Code mode completo | Loops programaticos com SDK tipado |
| B3 | Branch racing | Busca paralela de solucoes verificadas |
| B4 | Subagentes especializados | Planner/explorer/implementer/reviewer/verifier |
| B5 | Capability ledger | Zero-trust auditavel por capability |
| B6 | MCP firewall | MCP seguro por server/tool/data provenance |
| B7 | Memoria operacional | Projeto/time/procedimento aprendidos |
| B8 | Auto-skill mining | Traces viram skills versionadas |
| B9 | Trace-to-eval | Uso real vira regressao e dataset |
| B10 | Multi-model routing | Melhor modelo por papel, custo e risco |
| B11 | Continuous learning gate | Release gate por replay |
| B12 | Superiority gate | Prova numerica contra harness linear |

## Sprint B0 - Superiority baseline e eval harness

### Objetivo do sprint

Definir como o Navi provaria que ficou melhor que o baseline linear e melhor que a paridade.

### Epics

- Criar harness de eval local com tarefas versionadas.
- Separar suites:
  - simple repo tasks
  - long-horizon tasks
  - security stress tasks
  - MCP stress tasks
  - multi-file refactors
  - flaky/recovery tasks
- Medir baseline:
  - Navi parity mode
  - Navi verifier-first mode
  - Navi branch-race mode, quando existir
- Criar schema `EvalCase`.
- Criar comando headless para replay.

### Entregas

- `EvalCase` versionado.
- `EvalRun` com metrics.
- Primeira suite pequena de 10-20 casos.

### Definition of Done

- `just test-crate navi-core`
- Pelo menos um eval falha de forma reproduzivel quando a solucao esta incorreta.
- Traces conseguem gerar eval candidates.

### Objetivo para o proximo sprint

Usar os evals para medir reducao de chamadas de tools com repo intelligence.

## Sprint B1 - AST, LSP e code graph

### Objetivo do sprint

Fazer o Navi entender repositorios como grafo de simbolos, imports, chamadas e testes, nao apenas como texto.

### Epics

- Criar crate ou modulo `repo_intelligence` em `navi-core` ou crate dedicado.
- Tools:
  - `ast.search`
  - `symbol.goto`
  - `symbol.references`
  - `dependency_graph.query`
  - `test_discovery`
  - `ownership/churn.query` se git metadata permitir.
- Comecar com Rust e TypeScript/JavaScript, depois abrir para outras linguagens.
- Usar parsers estruturados quando possivel.
- Cache incremental por file hash.
- Expor consultas via SDK.

### Comparacao obrigatoria

- Claude: `LSPTool` no catalogo local.
- Codex: verificar se ha equivalentes de repo/context manager e tool routing por ambiente.
- Navi: `RepoExploreTool`, `CodeReadTool`, `CodeEditTool`, `SearchTool`.

### Entregas

- Symbol index local.
- `test_discovery` sugere o menor comando `just` aplicavel quando possivel.
- Verifier router usa test discovery.

### Definition of Done

- `just test-crate navi-core`
- Evals mostram reducao de `grep/read` em pelo menos uma classe de tarefa.
- Output do AST tool e compacto e redigido.

### Objetivo para o proximo sprint

Dar ao code mode um SDK tipado que use repo intelligence sem encher o contexto do modelo.

## Sprint B2 - Code mode completo com SDK tipado

### Objetivo do sprint

Permitir que loops, filtros, retries e transformacoes rodam fora do contexto do modelo, mas ainda sob policy, verifier e trace do Navi.

### Epics

- Expandir `code.exec` para runtime persistente com SDK:
  - `navi.repo.search`
  - `navi.repo.read`
  - `navi.repo.patch`
  - `navi.ast.search`
  - `navi.verify.run`
  - `navi.memory.read/write`
  - `navi.trace.note`
- Nested tool calls passam pelo mesmo `ToolExecutor`.
- Cada nested tool tem capability, effect report e trace.
- Quotas por cell e por session.
- `execute_to_pending` equivalente para detectar frontier bloqueada.
- Serializar scripts de code mode como artefatos reproduziveis.

### Comparacao obrigatoria

- Codex: `CodeModeService`, `CellActor`, nested tool call tests.
- Claude: `REPLTool` e tool concurrency declarations.

### Entregas

- `code.exec` pode iterar sobre N arquivos e aplicar patches pequenos com verifier.
- Nested tools sao visiveis no trace.
- Erros de script incluem remediation hints.

### Definition of Done

- `just test-crate navi-core`
- Tests para estado persistente, sessao isolada, nested approval, timeout e cancel.

### Objetivo para o proximo sprint

Usar snapshots/worktrees para testar multiplas solucoes em paralelo.

## Sprint B3 - Branch racing verificavel

### Objetivo do sprint

Fazer o Navi buscar no espaco de solucoes: criar hipoteses, executar em worktrees/snapshots, verificar e escolher por evidencia.

### Epics

- Tool/API `branch_race.start`.
- Gerar hipoteses:
  - minimal fix
  - test-first fix
  - refactor-safe fix
  - rollback/revert strategy quando aplicavel
- Criar worktree ou snapshot isolado por branch.
- Rodar subagente/implementer por branch com budget.
- Rodar verifiers e risk scoring.
- Score:
  - tests pass
  - typecheck/lint pass
  - diff size
  - effect risk
  - coverage delta
  - performance budget
  - user constraints
- Produzir `BranchRaceReport`.
- Merge supervisionado da branch vencedora.

### Comparacao obrigatoria

- Claude: worktree tools e team/agent docs locais.
- Codex: multi-agent mode e tool exposure tests.
- Navi: snapshots/rollback do plano de paridade.

### Entregas

- Branch race MVP com 2-3 estrategias.
- Relatorio de alternativa vencedora e rejeitadas.
- Rollback/cleanup garantido para worktrees perdedoras.

### Definition of Done

- `just test-crate navi-core`
- Eval bug complexo melhora em `verified_success_rate` ou reduz regressao.
- Nenhum staged change do usuario e tocado sem consentimento.

### Objetivo para o proximo sprint

Especializar os agentes que participam do branch race e da revisao.

## Sprint B4 - Subagentes especializados e reviewer independente

### Objetivo do sprint

Separar papeis de agente para reduzir contaminacao de contexto e melhorar julgamento independente.

### Epics

- Profiles:
  - `planner`
  - `explorer`
  - `implementer`
  - `reviewer`
  - `verifier`
  - `security_reviewer`
  - `summarizer`
- Cada profile declara:
  - model preference
  - toolset
  - capability budget
  - context budget
  - max wall time
  - allowed output schema
- Reviewer recebe diff, spec e verifier output, nao a conversa inteira.
- Security reviewer recebe effects, capability ledger e sensitive diff.
- Handoff estruturado entre agentes.

### Comparacao obrigatoria

- Claude: AgentTool, Team tools, SendMessageTool e permissions docs.
- Codex: multi-agent v2 namespaces e doctor thread inventory.
- Navi: `SubagentTool` e `SubagentTranscript`.

### Entregas

- Reviewer independente obrigatorio em branch race.
- Subagent outputs com JSON schema.
- Parent trace agrega child traces por role.

### Definition of Done

- `just test-crate navi-core`
- Tests para toolset isolation, budget enforcement e schema output.

### Objetivo para o proximo sprint

Substituir approvals amplos por um ledger de capabilities auditavel.

## Sprint B5 - Capability Ledger

### Objetivo do sprint

Criar um modelo zero-trust onde cada acao consome uma capability com escopo, justificativa, expiracao e auditoria.

### Epics

- Definir taxonomy:
  - `repo.read`
  - `repo.write.src`
  - `repo.write.tests`
  - `repo.write.docs`
  - `repo.write.ci`
  - `repo.write.lockfile`
  - `secrets.read`
  - `network.github`
  - `network.npm`
  - `shell.safe`
  - `shell.privileged`
  - `mcp.<server>.<capability>`
- Cada tool declara required capabilities.
- Policy concede capabilities por session, turn, branch, subagent ou single call.
- Approval prompt pede capability, nao apenas comando.
- Ledger registra:
  - requested
  - granted/denied
  - consumed
  - expired
  - violated
- Guarded capabilities nunca sao auto-concedidas.

### Comparacao obrigatoria

- Navi: `SecurityPolicy`, `ApprovalRequest`, `ApprovalRisk`.
- Codex: approval cache e network approval.
- Claude: wildcard permission rules e auto/bypass/plan modes.

### Entregas

- `CapabilityLedger` em `navi-core`.
- Runtime events para capability lifecycle.
- Trace inclui capability use.

### Definition of Done

- `just test-crate navi-core`
- Tests para expiration, subagent scoping, denied capability e guarded capability.

### Objetivo para o proximo sprint

Aplicar a mesma filosofia zero-trust a MCP.

## Sprint B6 - MCP Firewall

### Objetivo do sprint

Permitir que MCP seja poderoso sem confiar em servidores e tool descriptions por default.

### Epics

- Allowlist por workspace e por server.
- Pin de command/path/version/hash quando disponivel.
- Manifest signing hook para futuro.
- Sanitizar tool descriptions contra prompt injection.
- Taint tracking de dados vindos de MCP:
  - `mcp_data`
  - `external_content`
  - `untrusted_instruction`
- Capability por MCP tool, nao por servidor inteiro.
- Output redaction e summarization por risk.
- Logs auditaveis por server/tool.
- Bloquear MCP tool que tenta se apresentar como system/developer instruction.

### Comparacao obrigatoria

- Claude: MCP client/server docs e approval flow local.
- Codex: deferred MCP, invalid tool rejection e tool search tests.
- Navi: `navi-mcp` connection and tool mapping.

### Entregas

- `McpFirewallPolicy`.
- MCP provenance no trace e nos tool results.
- Tainted content nao entra no system prompt sem sanitizacao.

### Definition of Done

- `just test-crate navi-mcp`
- `just test-crate navi-core`
- Red-team tests para malicious tool description e oversized output.

### Objetivo para o proximo sprint

Persistir conhecimento operacional do projeto e do time.

## Sprint B7 - Memoria operacional em camadas

### Objetivo do sprint

Fazer o Navi lembrar o que importa para executar melhor no mesmo repo/time, sem criar contexto inchado ou obsoleto.

### Epics

- Camadas:
  - session memory
  - project memory
  - user/team memory
  - procedural memory
- Memory entries com:
  - scope
  - source trace
  - confidence
  - expiry/staleness
  - verifier evidence
  - owner
- Retrieval por task, files touched, tools recentes e errors.
- Injection seletiva no prompt.
- Memory garbage collection/doc gardening.
- Comando para auditar/remover memoria.

### Comparacao obrigatoria

- Claude: `src/memdir/`, `findRelevantMemories.ts`, team memory prompts.
- Navi: session memory e compaction.
- Codex: message history/context manager se aplicavel.

### Entregas

- `MemoryStore` estruturado.
- Project memory aprende build/test commands verificados.
- Procedural memory registra workflows bem-sucedidos.

### Definition of Done

- `just test-crate navi-core`
- Tests para retrieval, expiry, redaction e injection budget.

### Objetivo para o proximo sprint

Converter procedimentos repetiveis em skills versionadas.

## Sprint B8 - Auto-skill mining

### Objetivo do sprint

Minerar traces bem-sucedidos para criar skills reutilizaveis, testadas por replay antes de ativar.

### Epics

- Detector de padroes:
  - mesma sequencia de tools
  - mesmo conjunto de arquivos
  - mesmo verifier
  - mesmo erro e correcao
- Gerar `SkillDraft`:
  - name
  - trigger
  - workflow
  - required tools/capabilities
  - verification steps
  - examples
- Replay da skill contra trace/eval antes de ativar.
- Versionar skills e permitir rollback.
- Skill registry integrado a `tool.search`.
- Human approval para publicar skill fora do projeto local.

### Comparacao obrigatoria

- Claude: skill system docs locais.
- Navi: skills config, active skills, prompt injection.
- Trace store do plano de paridade.

### Entregas

- Trace -> SkillDraft.
- SkillDraft -> Eval replay.
- Skill ativa somente apos passar thresholds.

### Definition of Done

- `just test-crate navi-core`
- Teste prova que uma skill ruim nao e ativada sem replay pass.

### Objetivo para o proximo sprint

Transformar traces em evals e datasets de melhoria.

## Sprint B9 - Trace-to-eval e dataset flywheel

### Objetivo do sprint

Fazer cada execucao do Navi gerar material reutilizavel para regressao, router training, permission tuning e future SFT/RL export.

### Epics

- Trace -> EvalCase.
- Trace -> preference pair:
  - successful trajectory vs failed trajectory
  - low-risk vs high-risk action
  - efficient vs wasteful tool route
- Trace -> negative example.
- Trace -> tool-router training row.
- Trace -> permission classifier row.
- Trace -> verifier outcome reward.
- Redaction e consent boundaries.
- Export JSONL versionado.

### Entregas

- `navi eval generate-from-traces` ou API equivalente.
- Replay suite cresce a partir de uso real.
- Dataset export sem secrets.

### Definition of Done

- `just test-crate navi-core`
- Redaction tests especificos para dataset export.
- Pelo menos 5 evals gerados de traces sintéticos ou fixtures.

### Objetivo para o proximo sprint

Usar metricas dos traces para escolher modelos por papel.

## Sprint B10 - Multi-model harness routing

### Objetivo do sprint

Tornar o Navi modelo-agnostico e benchmark-driven, escolhendo modelos por papel, custo, latencia e confiabilidade.

### Epics

- Roles:
  - planner
  - router
  - coder
  - reviewer
  - verifier judge
  - summarizer
  - memory miner
- Model scorecards por role:
  - success rate
  - verifier pass rate
  - tool call validity
  - cost
  - latency
  - retry rate
  - unsafe action rate
- Router escolhe modelo por task class e risk.
- Fallback automatico com trace.
- Config por projeto e por user/team.
- Provider-specific tool protocol continua encapsulado em providers.

### Comparacao obrigatoria

- Navi: provider catalog, `navi-providers`, `navi-openai`, thinking config.
- Codex/Claude: observar configuracoes e model override de agentes quando disponiveis.

### Entregas

- `ModelRouter`.
- Scorecard persistido.
- Subagentes podem usar modelos diferentes com policy.

### Definition of Done

- `just test-crate navi-core`
- `just test-crate navi-sdk`
- Eval prova fallback ou routing diferente por role.

### Objetivo para o proximo sprint

Fazer replay e learning gates protegerem releases do harness.

## Sprint B11 - Continuous learning e release replay gate

### Objetivo do sprint

Toda release do harness deve ser testada contra tarefas reais e traces historicos antes de ser considerada segura.

### Epics

- Replay engine:
  - deterministic fixtures
  - recorded tool outputs
  - live verifier mode
  - dry-run policy mode
- Regression categories:
  - tool schema regression
  - routing regression
  - permission regression
  - verifier regression
  - memory retrieval regression
  - MCP firewall regression
- Release gate:
  - required eval suites
  - allowed metric deltas
  - security zero-tolerance checks
- Reports em JSON e Markdown.

### Entregas

- `just harness-replay` ou recipe equivalente.
- Relatorio de regressao por release.
- Evals gerados por trace entram no gate.

### Definition of Done

- `just test-crate navi-core`
- Gate falha quando uma policy permite efeito proibido.
- Gate falha quando verified success cai alem do threshold.

### Objetivo para o proximo sprint

Consolidar tudo em um gate de superioridade com metricas claras.

## Sprint B12 - Better-than-parity gate

### Objetivo do sprint

Provar que o Navi ficou melhor que o harness linear/parity mode em tarefas longas, arriscadas ou recorrentes.

### Epics

- Comparar modos:
  - parity linear
  - verifier-first
  - AST-assisted
  - branch racing
  - memory/skill assisted
  - multi-model routed
- Medir:
  - `verified_success_rate`
  - `verified_success_per_1k_tokens`
  - `tokens_per_success`
  - `tool_calls_per_success`
  - `wall_time_per_success`
  - `rollback_success_rate`
  - `unsafe_action_block_rate`
  - `permission_prompts_per_task`
  - `regressions_caught_by_replay`
  - `skills_mined`
- Produzir relatorio final com decisoes:
  - o que vira default
  - o que fica experimental
  - o que volta para backlog

### Definition of Done

- `verified_success_per_1k_tokens` melhora contra parity mode.
- Security stress suite nao registra unsafe guarded effect auto-approved.
- Replay gate pega pelo menos uma regressao injetada artificialmente.
- Uma tarefa recorrente melhora com memoria ou skill miner.
- Branch racing vence o modo linear em pelo menos uma tarefa complexa sem aumentar risco.

### Objetivo final atingido

O Navi deixa de ser um wrapper de tools quando o fluxo normal passa a ser:

```text
task
-> classify task/risk
-> retrieve operational memory
-> route model/profile
-> discover phase-appropriate tools
-> inspect repo via graph before grep-heavy exploration
-> propose hypotheses
-> execute with capabilities and sandbox
-> verify each meaningful effect
-> race branches when uncertainty is high
-> review independently
-> choose by evidence
-> persist trace
-> mine evals/skills/memory
-> improve future runs
```

## Metricas principais

| Metrica | Por que importa |
|---|---|
| `verified_success_per_1k_tokens` | Mede resolver barato e comprovado |
| `verified_success_rate` | Mede qualidade real, nao auto-relato |
| `unsafe_guarded_effect_auto_approval_count` | Deve ser zero |
| `rollback_success_rate` | Mede recuperacao de erro |
| `trace_to_eval_conversion_rate` | Mede flywheel de aprendizado |
| `skill_reuse_success_rate` | Mede valor de procedural memory |
| `branch_race_win_rate` | Mede valor de search over solutions |
| `model_router_regret` | Mede se o router escolhe modelos ruins |
| `mcp_tainted_instruction_block_rate` | Mede firewall MCP |

## Ordem de execucao resumida

1. B0 - eval baseline.
2. B1 - AST/LSP/code graph.
3. B2 - code mode completo.
4. B3 - branch racing.
5. B4 - subagentes especializados.
6. B5 - capability ledger.
7. B6 - MCP firewall.
8. B7 - memoria operacional.
9. B8 - auto-skill mining.
10. B9 - trace-to-eval/dataset.
11. B10 - multi-model routing.
12. B11 - release replay gate.
13. B12 - superiority gate.

