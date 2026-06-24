# NAVI × State-of-the-Art Harness — Análise de Pontos Fracos

> Cruzamento do relatório `harness-engineering-report-2026.md` com a implementação
> atual do NAVI. Cada gap referencia arquivos e linhas para tornar a ação trivial.
>
> **Escopo lido:** `crates/navi-core/src/{harness,turn,runtime,security,compact,prompt,skills}.rs`,
> `crates/navi-core/src/tool/builtin/{subagent,plan,top_files,bash,fs_browser,...}.rs`,
> `crates/navi-sdk/src/engine.rs`, `crates/navi-tui/src/...` (alto nível).
>
> **Avaliação geral:** NAVI tem um loop ReAct bem instrumentado, boa política de
> stop, compactação eficaz, aprovação por risco, prompt cache para KV-cache
> e suporte nativo a subagentes em background. Esses pontos são reais e o
> relatório anterior os destaca. **Mas o harness atual foi desenhado para
> turnos únicos ou poucas dezenas de interações** — e o estado da arte de 2026
> resolveu vários problemas que NAVI ainda não enxerga. Abaixo, os 28 gaps
> ordenados por impacto, cada um com referência concreta.

---

## Resumo executivo

| Categoria | Gaps | Impacto |
|---|---|---|
| Long-horizon autonomy | 5 | **Crítico** — sem caminho para apps grandes |
| Multi-agent / verificação | 5 | **Alto** — sem GAN-style, sem LLM-as-judge |
| Context engineering | 6 | **Alto** — sem AGENTS.md-as-TOC, sem recitação |
| Tool design | 4 | **Médio** — tool sprawl, sem scoping |
| Tool execution / browser | 3 | **Alto** — sem Playwright, sem lint agent-friendly |
| Operacional / métricas | 3 | **Médio** — sem evals, sem hit-rate visível |
| Subagente | 2 | **Médio** — sem override de modelo, prompt rígido |
| **Total** | **28** | — |

**Nenhum gap é bloqueador para o uso atual** (assistente de terminal one-shot
ou poucas dezenas de interações). Todos viram críticos quando o requisito é
autonomia multi-hora ou geração de apps completos.

---

## Categoria 1 — Long-horizon autonomy (Crítico)

### 1.1 Sem padrão Initializer + Coding agent

**Estado da arte:** Anthropic `effective-harnesses-for-long-running-agents` —
um agente `initializer` cria `init.sh`, `feature_list.json` e primeiro commit;
cada `coding` agent subsequente lê o estado, escolhe **uma** feature, implementa,
auto-verifica e commita limpo.

**NAVI atual:** `turn/mod.rs:92-140` (`run_turn`) é um loop monolítico
com `max_turn_loops` configurável (default até 150). Não há decomposição
em features, sem progress file, sem commit-per-feature, sem handoff
estruturado entre sessões.

**Consequência:** Em uma sessão longa, o agente tenta "one-shot" o app,
perde coerência, deixa o ambiente sujo. Em `record_tool_call`
(`harness.rs:350-389`), a única proteção contra "too much" é o `max_tool_calls`,
que simplesmente para a execução em vez de persistir e retomar.

**Ação:** Implementar `InitializerAgent` + `CodingAgent` profiles em
`harness.rs`, criar artefatos `feature_list.json` e `navi-progress.txt`
via tool `init_session`, e expor `start_long_running_session()` no SDK.

### 1.2 Sem `feature_list.json` + sprint contract

**Estado da arte:** Features são primeira-classe com `passes: false/true`,
marcadas **apenas** após verificação. Contratos negociados antes de
implementar.

**NAVI atual:** Existe `tool/builtin/plan.rs` (757 linhas), o system prompt
diz "create a plan FIRST using the `plan` tool" (`harness.rs:280-286`),
mas é só um hint para o modelo. Sem enforcement, sem critério verificável
de "done", sem JSON machine-readable de feature status. O modelo pode
criar um plano e ignorá-lo, ou marcar como concluído sem testar.

**Ação:** Substituir o `plan` tool (Markdown livre) por uma versão que
emite JSON versionado com `passes: bool`, `verification_steps: [...]`,
e tool `mark_feature_done` que **só** aceita após execução de
`verification_steps` (ex.: `cargo test`, `curl`, `playwright_navigate`).

### 1.3 Sem context reset estruturado

**Estado da arte:** Anthropic descobriu que compaction preserva continuidade
mas não resolve "context anxiety" (modelo enrola pra terminar). Solução:
context reset + handoff artifact. Manus usa o mesmo padrão.

**NAVI atual:** Compactação in-place em `compact.rs:174-280` resume em 9 seções
e mantém a porcentagem `keep_ratio` (default 25%) intacta. Não há reset
limpo. `start_session` (`runtime/mod.rs:423-443`) reseta mas perde
histórico (vai pra `SessionStore` via `snapshot_session`).

**Consequência:** Modelos com "context anxiety" (Sonnet 4.5) não são
tratados. Para Opus 4.5 a Anthropic abandonou resets, mas NAVI não tem
essa opção configurável.

**Ação:** Adicionar `HarnessStopReason::ContextResetNeeded` e um modo
`profile = "long-running"` que, ao bater limite, serializa estado para
`.navi/handoff-<session>.md` e reseta `messages` mantendo apenas o handoff.

### 1.4 Hard turn loop cap de 150 sem fallback

**Estado da arte:** 150 iterações é o `HARD_TURN_LOOP_LIMIT` em
`turn/mod.rs:23`. Nenhum harness de produção famoso usa 150 — Anthropic
trabalha com **uma feature por sessão**, então o loop é tipicamente 10-30.
OpenAI Codex CLI também opera em loops curtos.

**Consequência:** O cap de 150 esconde problemas. Modelos podem entrar
em loops longos que seriam interrompidos antes com resets. Logs do
`tracing::info!` em `turn/mod.rs:269-274` ficam ruidosos.

**Ação:** Reduzir default para 30-50 e expor perfil `extended` (150)
para casos específicos. Sempre que o cap for atingido, emitir
`HarnessTrace` estruturado com `last_n_tool_signatures` para diagnóstico.

### 1.5 Sem doc-gardening / entropy management

**Estado da arte:** OpenAI (fev 2026) tem agentes recorrentes que varrem
docs obsoletas. Martin Fowler formaliza como "entropy management" —
periodic garbage collection para evitar drift de knowledge base.

**NAVI atual:** `AGENTS.md` é lido em `prompt.rs:108-111` e cacheado,
mas não há agente ou task que detecte drift. O sistema não tem
mecanismo de "staleness check".

**Ação:** Adicionar tool `doc_gardening(scan_paths)` que compara
`AGENTS.md`, `docs/`, `ARCHITECTURE.md` contra o estado real do código
(diff de symbols, signatures) e retorna uma lista de desatualizações.
Pode ser invocado como skill opcional.

---

## Categoria 2 — Multi-agent e verificação (Alto)

### 2.1 Sem generator/evaluator GAN-style

**Estado da arte:** Anthropic "Harness design for long-running apps"
separa generator e evaluator. Evaluator recebe Playwright MCP,
navega o app real, aplica critérios ponderados, devolve feedback
estruturado. Em 4h de wall-clock, executa 5-15 iterações.

**NAVI atual:** Existe `SubagentTool` (`tool/builtin/subagent.rs:23-562`)
mas o "subagente" é **mais do mesmo modelo** rodando um loop isolado.
Não há agente com persona diferente (planner, evaluator, critic).

**Consequência:** O modelo é juiz de si mesmo. Em tarefas subjetivas
(design, qualidade) e até em tarefas verificáveis, o auto-julgamento
é generoso.

**Ação:** Adicionar campo `persona: "generator" | "evaluator" | "planner"`
em `subagent`. Persona `evaluator` recebe tools de leitura + browser,
e system prompt com critérios ponderados. Persona `planner` recebe
apenas read tools + plan tool.

### 2.2 Sem LLM-as-judge pós-tool

**Estado da arte:** Após cada write tool, harness executa LLM-as-judge
rápido: "esse diff está coerente com o spec?". Em 83% → 96% task
completion com verification layer (Vercel).

**NAVI atual:** Nada. `record_tool_result` (`harness.rs:393-462`) só
classifica falha; sucesso passa direto.

**Ação:** Adicionar hook opcional `post_write_judge: bool` em
`HarnessConfig`. Quando ativo, após `apply_patch`/`write_file`,
spawn evaluator subagente com contexto limitado ao diff, que
retorna `{verdict: "ok" | "needs_rework", feedback: "..."}`.

### 2.3 Sem browser automation (Playwright/Puppeteer)

**Estado da arte:** Anthropic e OpenAI tratam browser como sensor
obrigatório para apps web. Playwright MCP é commodity.

**NAVI atual:** `grep playwright|puppeteer|chromedevtools` em
`crates/` retorna zero matches. TUI tem `browser.rs` (renderização
de HTML para visualização, não automation) e `chrome-devtools-mcp`
existe no ambiente do agente mas não está integrado.

**Ação:** Adicionar tool `browser` com actions `navigate`, `screenshot`,
`click`, `fill`, `eval`. Implementar como thin wrapper sobre
`chromedevtools` MCP server (já disponível). Registrar como
`ToolKind::Command` no security policy.

### 2.4 Subagente auto-aprova as próprias actions

**Estado da arte:** Subagentes em Anthropic/OpenAI escalam aprovações
para o orquestrador, ou têm um policy restrito. Aprovar sozinho é
vetado.

**NAVI atual:** `subagent.rs:552-558` — quando um subagente emite
`ApprovalRequested`, o próprio `event_rx` task resolve
**automaticamente** com `ApprovalDecision::Approved`. Nenhum prompt
ao usuário.

**Consequência:** Em YOLO mode é equivalente; sem YOLO é perigoso —
writes e comandos executam sem confirmação. A lógica parece assumir
que subagentes herdam aprovações, mas não é o caso se o subagente
foi spawnado para algo arriscado.

**Ação:** Trocar `ApprovalDecision::Approved` por um canal que escala
para o `approval_resolver` do parent. Adicionar config
`subagent.approval_mode: "inherit" | "yolo" | "escalate"`.

### 2.5 Subagente sem model override

**Estado da arte:** Codex (Mar 2026) e Cursor permitem subagentes
com modelo próprio (explorer usa mini, implementer usa full).
Vantagem: economia, especialização.

**NAVI atual:** `subagent.rs:35-57` — `model_provider` e `model_name`
são clones do parent, sem override. `SubagentTool` herda 100% do
modelo principal.

**Ação:** Adicionar campo `model: Option<String>` no input de
`subagent`. Se presente, o subagente cria um `TurnContext` com
model_provider alternativo.

---

## Categoria 3 — Context engineering (Alto)

### 3.1 `AGENTS.md` carregado integralmente

**Estado da arte:** OpenAI (fev 2026) testou AGENTS.md de 1000 linhas,
falhou em 4 modos previsíveis. Solução: `AGENTS.md` curto (~100 linhas)
como TOC, knowledge real em `docs/` com progressive disclosure.

**NAVI atual:** `prompt.rs:108-126` — lê `AGENTS.md` integralmente
e concatena com o system prompt. Sem cap de tamanho. Sem diretório
`docs/` ou similar. Sem `AGENTS.md` com cap. Nada força disciplina.

**Ação:** Adicionar config `ag.agents_md_max_lines` (default 200).
Carregar `docs/ARCHITECTURE.md` e `docs/QUALITY_SCORE.md` apenas como
links referenciais. Warning se `AGENTS.md` passar do cap.

### 3.2 Sem recitação de objetivos (todo.md)

**Estado da arte:** Manus mantém um `todo.md` re-citado a cada ~50
tool calls para manter objetivos no fim do attention span. Sem isso,
agentes drift após longos turnos.

**NAVI atual:** O `plan` tool emite plano, mas não há policy que
force a recitação. A cada turno, o modelo pode esquecer o plano
após 50+ tool calls. Não há `todo.md` automático.

**Ação:** Adicionar contador `tool_calls_since_plan_recite`. A cada
40 calls, o harness injeta `<plan_recitation_reminder>` no system
prompt e exige que o modelo atualize o plan via tool call antes
de prosseguir.

### 3.3 Skills carregadas eagerly, sem progressive disclosure

**Estado da arte:** LangChain "Anatomy of an Agent Harness" — Skills
são primitive de progressive disclosure. Metadata no contexto sob
demanda, instruções completas carregadas apenas quando ativadas.

**NAVI atual:** `skills.rs:94-121` — `render_active_skills` inclui
id, name, description, version, author, tags, requires, **e** as
instruções completas de cada skill ativa. Se o usuário tem 10 skills
ativas, todas as instruções vão no system prompt.

**Ação:** Refatorar para dois estágios: (1) sempre: id, name, description;
(2) sob demanda: tool `load_skill(id)` que retorna as instruções e
marca a skill como "loaded" no `TurnContext`.

### 3.4 Sem re-injeção de contexto durante a sessão

**Estado da arte:** OpenAI sugere doc-gardening agents que re-injetam
conhecimento ao longo da sessão, não só no início.

**NAVI atual:** `ContextPackets` são injetados em `ensure_system_prompt`
(`turn/mod.rs:150-182`) que roda **uma vez** no início do turno. Skills
são resolvidas em `load_active_skills` no `start_session`. Nada
atualiza esses durante o turno.

**Ação:** Adicionar `maintain_dynamic_context()` em
`maintain_context_budget` (`turn/mod.rs:184-230`). Em pontos
naturais (compact, halfway through max_tool_calls), recarregar
skills/packets de fontes que podem ter mudado (file watcher,
git status).

### 3.5 Compact não preserva status do plan como primeira-classe

**Estado da arte:** OpenAI Claude Code mantém o plan status como
artifact versionado. Compact deve preservar IDs e completion
status.

**NAVI atual:** `compact.rs:317-348` — `COMPACT_PROMPT` menciona
"Active Work Plan" como seção 9, mas o `auto_compact` substitui
a conversation por um summary textual. O plan tool (que tem
estado machine-readable) é apenas texto no prompt, sem link
forte entre o summary e o plan file.

**Ação:** Adicionar referência explícita ao `plan_file` (criado
pelo plan tool) no summary. Após compact, o harness injeta
`<plan status="X" source=".navi/plan-<id>.json">` no system
prompt para que o modelo possa consultar via `read_file`.

### 3.6 Sem métrica de KV-cache hit rate

**Estado da arte:** Manus considera hit rate de KV-cache como KPI
#1 de produção. Cached tokens são 10x mais baratos (Claude Sonnet).

**NAVI atual:** `PromptCache` (`prompt.rs:19-95`) cacheia AGENTS.md e
tool manifest, mas a `PromptCache::disk_read_count` é só para
file cache. Não há `cache_hit`/`cache_miss` exposto no `HarnessTrace`.
O `HarnessTrace` em `turn/mod.rs:262-275` emite
`trace_request_summary` que lista `messages` count, mas não o
quanto do prefixo é cacheable.

**Ação:** Adicionar `cache_read_tokens` e `cache_creation_tokens`
já reportados pelo provider (ver `turn/mod.rs:367-387`) no
`HarnessTrace`. Expor `cache_hit_ratio` no Debug modal do TUI
(`view.rs` e `dispatch.rs`). Alertar se ratio cai abaixo de
threshold (e.g., 0.7).

---

## Categoria 4 — Tool design (Médio)

### 4.1 Tool sprawl sem scoping por fase

**Estado da arte:** Vercel removeu 80% das tools e melhorou resultados.
Menos tools = menos confusão. Tools por fase do task (planning,
implementation, verification) escopadas dinamicamente.

**NAVI atual:** 18 tools built-in (`ls crates/navi-core/src/tool/builtin/`),
mais MCP, mais plugins. Todas disponíveis em todos os turnos.
`build_model_request` (`turn/mod.rs:232-260`) envia o array completo
de `tool_executor.definitions()`.

**Ação:** Implementar `ToolSet::for_phase(Phase)` em
`tool/mod.rs`. Phase `planning` retorna `read_file`, `grep`,
`fs_browser`, `top_files`, `plan`. Phase `implementing` adiciona
`write_file`, `apply_patch`, `bash`, `test_runner`, `build_runner`.
Phase `verifying` adiciona browser. Adicionar `current_phase` ao
`TurnContext` controlado pelo harness.

### 4.2 Tool descriptions não seguem "agent UX" guide da Anthropic

**Estado da arte:** Anthropic "Writing Effective Tools for Agents" —
tool design é agent UX. Nome legível, schema estrito, error messages
projetadas para LLM, idempotência, token budget declarado.

**NAVI atual:** Schema OK na maioria. **Error messages são mensagens
de runtime** (e.g., "file not found" em `tool/builtin/read.rs`),
não mensagens projetadas para o modelo — não incluem remediation
hints nem instrução de retry.

**Ação:** Padronizar error schema em `tool/mod.rs`:
```json
{
  "ok": false,
  "error_code": "file_not_found",
  "message": "human readable",
  "remediation": "use fs_browser to list the directory first, or check the path",
  "retryable": true
}
```

### 4.3 Sem idempotência explícita em write tools

**Estado da arte:** Tool chamada duas vezes tem o mesmo efeito.
Especialmente importante em retries automáticos.

**NAVI atual:** `write_file` é write-destrutivo sem idempotency
key. `apply_patch` pode duplicar conteúdo se chamado duas vezes
idênticas (o repetition detector em `harness.rs:376-386` para
após 20 calls, mas dentro disso tudo executa).

**Ação:** Adicionar `idempotency_key` opcional em tool invocations.
Harness dedup por `tool_name + idempotency_key` em janela curta.

### 4.4 Sem token budget declarado nos tools

**Estado da arte:** Tools declaram `output_max_tokens` para o harness
poder budgetar.

**NAVI atual:** Tools não declaram. `observation_max_bytes` é global
em `HarnessConfig`. Não há per-tool limit.

**Ação:** Adicionar `output_budget_bytes` no `ToolDefinition`. Harness
trunca outputs específicos por tool (e.g., `bash` pode pedir mais
que `read_file`).

---

## Categoria 5 — Tool execution e browser (Alto)

### 5.1 Sem linter agent-friendly

**Estado da arte:** OpenAI codifica "remediation instructions para o
agente" em mensagens de linter customizado. Linter vira **feedback
sensor inferencial-computacional**.

**NAVI atual:** Não há linter. NAVI invoca `cargo test`, `cargo clippy`
via `bash` ou `test_runner`/`build_runner`, mas sem wrapping que
injete remediation hints no contexto.

**Ação:** Adicionar tool `lint` com detecção de projeto (cargo/npm/etc.)
que executa linter, captura output, e re-escreve erros em formato
`{rule, file, line, message, remediation: "Add a test case that..."}`.

### 5.2 Sem auto-run de tests/build após writes

**Estado da arte:** Anthropic frontend-design harness roda browser
tests após cada sprint. OpenAI shipping 1M LOC roda CI em worktree
ephemeral.

**NAVI atual:** O modelo precisa **lembrar** de chamar
`test_runner`/`build_runner` após `apply_patch`. System prompt
diz "After writes, verify" (`harness.rs:256`), mas é só instrução,
não enforço.

**Ação:** Adicionar hook `post_write_verify: bool` em
`HarnessConfig`. Após `apply_patch`/`write_file` em Rust/TS/Go
files, harness automaticamente invoca `test_runner` com escopo
do arquivo modificado, injeta resultado no contexto.

### 5.3 Sem browser tool de fato (cf. 2.3)

Repetido para checklist. **Crítico** para apps web.

---

## Categoria 6 — Operacional e métricas (Médio)

### 6.1 Sem eval harness

**Estado da arte:** Anthropic "Demystifying Evals for AI Agents" —
unit-test evals falham para agents. Precisam de trajectory evals
e harness ablation studies.

**NAVI atual:** Apenas unit tests em `crates/*/src/*/tests.rs` para
lógica de harness. Nenhum eval de comportamento do agente
(tipo SWE-bench, HumanEval, ou trajectory replay).

**Ação:** Adicionar `crates/navi-evals/` com harness que:
- Carrega tarefas de `evals/tasks/*.yaml`
- Executa turn em runtime com modelo/provider fixo
- Grava trajectory (todos os `AgentEvent`s) em JSON
- Compara contra trajectory esperada (flex match)

### 6.2 Sem métricas de harness effectiveness

**Estado da arte:** Anthropic 2026 Agentic Coding Trends Report —
harness setup sozinho move benchmarks em 5+ pp. Métricas como
"tool call success rate", "session completion rate", "context
utilization" são vitais.

**NAVI atual:** TUI Debug modal mostra provider/model/session ID,
mas não tem painel de métricas ao longo da sessão. Eventos
`HarnessTrace` existem mas não são agregados.

**Ação:** Adicionar `HarnessMetrics` agregados no `SessionState`:
- `total_tool_calls`, `success_rate`, `avg_iterations_to_completion`
- `context_peak_pct`, `compact_count`, `circuit_open_count`
- `approval_request_count`, `denial_count`
Expor no TUI Debug modal e em `RuntimeEvent::MetricsUpdated`.

### 6.3 Sem continuous drift detection

**Estado da arte:** Martin Fowler "Keep quality left" — drift
sensors rodando fora do change lifecycle (dead code, dependency
scanners, quality monitoring).

**NAVI atual:** Nenhum. NAVI é completamente reativo a tool calls.

**Ação:** Adicionar task opcional `drift_check` que roda em idle
(sem tool call por N segundos) e emite warnings sobre
`AGENTS.md` drift, skill staleness, doc freshness.

---

## Categoria 7 — Subagente (Médio)

### 7.1 Sem override de profile/policy

**Estado da arte:** Subagentes podem ter `HarnessPolicy` próprio —
mais restritivo (read-only) para explorer, permissivo para implementer.

**NAVI atual:** `subagent.rs:288-291` — usa
`policy_for_profile(&self.harness_config, self.harness_config.profile)`,
herdando 100%.

**Ação:** Aceitar `profile: "small" | "medium" | "readonly"` no input.
Profile `readonly` desabilita write/command tools no `ToolExecutor`
do subagente.

### 7.2 Prompt hardcoded de subagente

**Estado da arte:** Subagentes devem receber persona e constraint
claros, e idealmente compartilharem o "map" do projeto.

**NAVI atual:** `subagent.rs:508-523` — system prompt é
"You are a subagent worker. Execute the assigned task autonomously
using whatever tools are needed." Genérico, sem skills, sem
memória do parent, sem AGENTS.md.

**Ação:** Subagente deve herdar: `AGENTS.md` (cacheado via
`PromptCache` compartilhado), `active_skills` filtradas por
`requires`, e opcionalmente um `context_packet` do parent.

---

## Categoria 8 — Issues de código/arquitetura (Miscelânea)

### 8.1 `top_files` referenciado mas talvez redundante com `fs_browser`

`harness.rs:260-261` recomenda `top_files` para first-pass
exploration, e `top_files.rs` existe (778 linhas), mas o
system prompt não explica quando usar `top_files` vs
`fs_browser(list/tree)`. Reduzir a ambiguidade de tool choice.

### 8.2 Subagente background max 8 sem rate limiting

`subagent.rs:21` e `subagent.rs:329-330` — `MAX_BACKGROUND_SUBAGENTS = 8`.
Se o modelo spawna 8 background tasks, todas consomem providers
paralelos sem rate limit. Pode disparar 429 ou burst budget.

**Ação:** Adicionar concurrency limiter global (semaphore com
N = max_parallel_tool_calls) e rate limit por janela.

### 8.3 `_plugins: Vec<LoadedPlugin>` no NaviSession

`engine.rs:131` — `LoadedPlugin`s são carregados no startup mas
o campo tem underscore (`_plugins`), indicando "guardado mas não
usado na runtime". Verificar se plugins estão sendo realmente
roteados para o tool executor ou se é vazio.

---

## Matriz de prioridade (impacto × esforço)

| Gap | Impacto | Esforço | Prioridade |
|---|---|---|---|
| 1.1 Initializer + Coding agent | Crítico | Alto | **P0** (transforma NAVI) |
| 1.2 feature_list.json + sprint contract | Crítico | Médio | **P0** |
| 2.3 Browser automation (Playwright) | Alto | Médio | **P1** |
| 2.4 Subagente auto-approval | Alto | Baixo | **P1** (segurança) |
| 3.1 AGENTS.md-as-TOC + docs/ | Alto | Baixo | **P1** |
| 3.2 todo.md recitation | Alto | Médio | **P1** |
| 3.3 Skills progressive disclosure | Alto | Médio | **P1** |
| 3.6 KV-cache hit rate metric | Médio | Baixo | **P2** |
| 4.1 Tool scoping por phase | Médio | Médio | **P2** |
| 5.1 Linter agent-friendly | Médio | Médio | **P2** |
| 5.2 Auto-run tests após write | Médio | Médio | **P2** |
| 2.2 Post-write judge | Alto | Alto | **P2** |
| 2.1 Generator/Evaluator persona | Alto | Alto | **P3** |
| 1.3 Context reset + handoff | Médio | Alto | **P3** |
| 6.1 Eval harness | Médio | Alto | **P3** |
| 6.2 Harness metrics | Médio | Médio | **P3** |
| 7.1 Subagente readonly profile | Médio | Baixo | **P3** |
| 4.2 Tool error remediation hints | Médio | Médio | **P3** |
| 4.3 Idempotency keys | Baixo | Médio | **P4** |
| 4.4 Per-tool token budget | Baixo | Médio | **P4** |
| 1.4 Reduzir hard loop cap | Baixo | Trivial | **P4** |
| 1.5 Doc gardening | Médio | Médio | **P4** |
| 2.5 Subagente model override | Baixo | Baixo | **P4** |
| 3.4 Re-inject context dinâmico | Baixo | Médio | **P4** |
| 3.5 Plan status em compact | Baixo | Médio | **P4** |
| 7.2 Subagente herda skills/AGENTS | Baixo | Baixo | **P4** |
| 6.3 Drift detection | Baixo | Alto | **P5** |
| 8.x Miscelânea | Baixo | Baixo | **P5** |

---

## Recomendações de roadmap

**Fase 1 — Quick wins (P1+P2, ~4-6 semanas):**
1. Cap `AGENTS.md` em 200 linhas, forçar `docs/` como knowledge base
2. Subagente com `readonly` profile e override de modelo
3. Auto-run tests após write em projetos detectados
4. Linter agent-friendly wrapper
5. KV-cache hit rate no Debug modal
6. Fix subagente auto-approval
7. Skills progressive disclosure (metadata always, body on demand)
8. todo.md recitation policy

**Fase 2 — Long-horizon (P0, ~8-12 semanas):**
1. Initializer + Coding agent profile com feature_list.json
2. Browser tool (Playwright wrapper)
3. Post-write judge persona

**Fase 3 — Operacional (P3, ongoing):**
1. Eval harness
2. Generator/Evaluator personas
3. Harness metrics agregados

---

## Apêndice — Mapa dos arquivos relevantes

```
crates/navi-core/src/
  harness.rs           # HarnessPolicy, AgentRunState, stop reasons
  turn/mod.rs          # run_turn loop principal
  runtime/mod.rs       # AgentRuntime, send_turn, lifecycle
  compact.rs           # micro_compact + auto_compact
  prompt.rs            # PromptCache, SystemPromptRenderer
  skills.rs            # SkillManifest, discovery, render
  security.rs          # SecurityPolicy, approval flow
  tool/builtin/
    subagent.rs        # SubagentTool (foreground + background)
    plan.rs            # plan tool
    top_files.rs       # first-pass exploration
    bash.rs, read.rs, write.rs, apply_patch.rs, ...
  config/types.rs      # HarnessConfig, HarnessProfile, etc.

crates/navi-sdk/src/
  engine.rs            # NaviEngine API pública
```

## Apêndice — Correspondência com o relatório de harness

| Relatório | Implementação NAVI | Status |
|---|---|---|
| Initializer + Coding (Anthropic) | — | **Falta** |
| Planner + Generator + Evaluator (Anthropic) | SubagentTool básico | **Parcial** |
| Ralph Loop (Huntley) | — | **Falta** |
| Subagentes paralelos (Codex) | SubagentTool | **Parcial** (sem model override) |
| AGENTS.md-as-TOC (OpenAI) | AGENTS.md integral | **Fraco** |
| Custom linters com remediation (OpenAI) | — | **Falta** |
| Browser automation (Playwright) | — | **Falta** |
| Generator/evaluator separação (Anthropic) | — | **Falta** |
| Skills progressive disclosure (LangChain) | Eager loading | **Fraco** |
| doc-gardening (OpenAI) | — | **Falta** |
| KV-cache hit rate (Manus) | PromptCache local | **Parcial** |
| tool scoping por phase (Vercel) | — | **Falta** |
| LLM-as-judge (Vercel) | — | **Falta** |
| Compact + circuit breaker (NAVI) | `compact.rs:128-136` | **OK** |
| Tool call repetition detection (NAVI) | `harness.rs:376-386` | **OK** |
| Parallel tool calls (NAVI) | `turn/mod.rs:549-555` | **OK** |
| Provider-correct transcripts (NAVI) | `prompt.rs:113`, `turn/mod.rs:232-260` | **OK** |
| Security policy + approval flow (NAVI) | `security.rs`, `turn/mod.rs:743-795` | **OK** |
| Question tool (NAVI) | `tool/builtin/question.rs` | **OK** |

**Resumo de cobertura:** NAVI tem **6 de 19 padrões state-of-the-art** (≈32%).
Os 13 restantes são gaps ativos. Para chegar a paridade com Claude Code/Codex,
foco em P0 e P1 entrega 70% do valor com 30% do esforço.

---

*Análise feita em junho 2026, baseada em leitura direta do código em
`/home/enrell/projects/navi/crates/navi-core/src/` e `crates/navi-sdk/src/`,
cruzada com o relatório `docs/harness-engineering-report-2026.md`.*
