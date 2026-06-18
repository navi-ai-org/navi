# Harness e Técnicas de Harness para Code Agents — Relatório 2026

> Pesquisa consolidada sobre o estado da arte em harness engineering para coding agents em 2026.
> Fontes principais: Anthropic, OpenAI, LangChain, Martin Fowler/ThoughtWorks, Morph, Manus, Cursor, Geoffrey Huntley e o repositório `ai-boost/awesome-harness-engineering`.

---

## 1. Resumo executivo

Em 2026, a frase que melhor captura o momento é **"o modelo é commodity, o harness é o moat"** (harness-engineering.ai). A Anthropic, a OpenAI, a ThoughtWorks e a Manus publicaram em poucos meses postagens técnicas que convergem no mesmo ponto: **a engenharia do harness — tudo que envolve o modelo sem ser o modelo — determina se um coding agent demo-ready se torna um coding agent production-ready**.

Pontos convergentes entre as fontes:

- **Harness = Model + Harness** (LangChain). Tudo o que não é peso do modelo entra no harness: prompts, ferramentas, sandbox, memória, hooks, orquestração, verificação.
- **O loop ReAct (Reason + Act) é universal**. Claude Code, Cursor, Codex CLI, Cline, Aider e Manus implementam a mesma estrutura; o que muda é como o harness gerencia cada passo.
- **Harness engineering eclipsou model selection como alavanca de performance** — a Anthropic (2026 Agentic Coding Trends Report) reporta que a configuração do harness sozinha pode mover benchmarks em **5+ pontos percentuais**.
- **Falhas em coding agents raramente são do modelo**. São de contexto (context rot, context anxiety), memória entre sessões, feedback loops fracos, ferramentas mal desenhadas ou ausência de verificação externa.
- **Long-horizon autonomy** exige decomposição em features, resets de contexto, handoff estruturado, planos editáveis e verificação independente.

---

## 2. Definições

### 2.1 O que é um harness

> "If you're not the model, you're the harness." — Vivek Trivedy (LangChain, 2026)

Concretamente, o harness inclui:

| Categoria | Exemplos |
|---|---|
| Prompts | System prompt dinâmico, `AGENTS.md`, regras do projeto |
| Ferramentas | Tool definitions, MCP servers, Skills, comandos de bash |
| Infraestrutura | Filesystem, sandbox, browser automation, observabilidade |
| Orquestração | Subagent spawning, handoffs, model routing, context resets |
| Middleware/Hooks | Compaction, lint checks, validação de output, retry policies |
| Estado | Memória entre sessões, progress files, checkpoints git |
| Verificação | Self-eval, LLM-as-judge, browser tests, linters customizados |

### 2.2 Frameworks conceituais de referência

Três frameworks dominantes estruturam o campo em 2026:

**IMPACT (swyx, Morph 2026):**
- **I** — Intent: objetivos verificáveis via evals
- **M** — Memory: long-running, skills, workflows reutilizáveis
- **P** — Planning: planos multi-step **editáveis** pelo usuário
- **A** — Authority: permissões, aprovação, sandbox
- **C** — Control Flow: o quanto o LLM decide vs. workflows fixos
- **T** — Tools: descoberta, escopo, execução

**Feedforward/Feedback (Böckeler, Martin Fowler 2026):**
- **Guides (feedforward)** antecipam o comportamento e conduzem o agente *antes* de agir
- **Sensors (feedback)** observam *depois* e permitem auto-correção
- **Computational** (linters, testes, type checkers) — determinístico, barato
- **Inferential** (LLM-as-judge, AI code review) — não-determinístico, caro
- Três categorias de regulação: **Maintainability**, **Architecture fitness**, **Behaviour**

**Seis componentes essenciais (harness-engineering.ai):**
1. Context engineering
2. Tool orchestration
3. State & memory management
4. Verification & safety
5. Human-in-the-loop controls
6. Lifecycle management

---

## 3. O Agent Loop — estrutura canônica

Pseudocódigo que descreve o loop de qualquer coding agent sério (Morph, validado por 60% dos 70 projetos open-source analisados pelo repositório `awesome-harness-engineering`):

```text
while task_not_complete:
    # 1. READ — gather relevant context
    state = read_files() + read_test_output() + read_errors()
    context = harness.select_context(state, task)

    # 2. PLAN — decide what to do next
    plan = model.reason(context, task)

    # 3. ACT — execute via tools
    result = harness.dispatch_tool(plan.next_action)

    # 4. OBSERVE — check the outcome
    outcome = harness.evaluate(result)
    if outcome.needs_retry:
        context = harness.add_error_trace(outcome.error)
        continue
    if outcome.needs_human:
        harness.escalate(outcome)
        break
    harness.checkpoint(result)  # git commit, progress file
```

**Insight crítico:** o loop não funciona sem feedback confiável de testes, linters e type checkers. "Agents sem suite de testes alucinam progresso" (Addy Osmani). O harness é o que torna o feedback denso, rápido e legível para o modelo.

---

## 4. Padrões de arquitetura de harness

### 4.1 Initializer + Coding agents (Anthropic, Nov 2025)

Para trabalho **long-running** que atravessa múltiplas janelas de contexto:

**Sessão 1 — Initializer agent:**
- Cria `init.sh` (ambiente bootável)
- Cria `claude-progress.txt` (log de progresso)
- Cria `feature_list.json` com **200+ features**, todas `passes: false`
- Faz commit inicial do git

**Sessões N — Coding agent:**
1. Lê `claude-progress.txt` e `git log` para se situar
2. Lê `feature_list.json` e escolhe **uma** feature de maior prioridade
3. Implementa a feature
4. Auto-verifica com browser automation (Puppeteer/Playwright MCP)
5. Marca `passes: true` **apenas** após verificação real
6. Atualiza `claude-progress.txt` e commita

Falhas abordadas por este padrão:
- Agente tentar one-shot o app inteiro → uma feature por sessão
- Agente declarar vitória prematura → só marca passando após teste
- Agente deixar o ambiente sujo → commit limpo e progress file
- Agente não saber testar → init.sh + browser tools explícitos

**Lição transferível para NAVI:** o padrão de feature list JSON + progress file + um feature por turno encaixa direto no nosso `TuiApp` e `SessionStore`.

### 4.2 Planner + Generator + Evaluator (Anthropic, Mar 2026)

Inspirado em **GANs**, este é o padrão state-of-the-art para long-running + design subjetivo.

```
User prompt (1-4 sentenças)
        ↓
   [PLANNER] → expande para spec ambiciosa, alto-nível
        ↓
   [GENERATOR] → implementa em sprints, uma feature por sprint
        ↓
   [EVALUATOR] → Playwright MCP, navega o app real,
                  valida bugs, grade em critérios ponderados
        ↓
   (se falhou) → feedback detalhado volta ao generator
        ↓
   (se passou) → próximo sprint, novo contract
```

**Sprint contracts:** antes de cada sprint, generator e evaluator **negociam** o que "done" significa, com critérios de aceite verificáveis. Isso fecha o gap entre user stories de alto nível e implementação testável.

**Critérios ponderados** (exemplo para frontend design):
- Design quality + Originality (peso alto) — Claude erra sem prompting
- Craft + Functionality (peso médio) — Claude já acerta por default
- Penalidade explícita a "AI slop" (gradientes roxos, white cards)

**Resultado:** iterações melhoram monotonicamente até platô, com "leaps" criativos raros entre ciclos. Runs chegam a 4h de wall-clock e 15 iterações.

### 4.3 Ralph Loop (Geoffrey Huntley)

Padrão "monolítico" e minimalista:

- **Intercepta a tentativa do modelo de sair** via hook
- **Reinjeta o prompt original** em uma janela de contexto limpa
- Força o agente a continuar até bater uma completion goal
- Filesystem é a única memória entre iterações

```bash
# Pseudo-implementação de Ralph
while ! check_done; do
    claude --fresh-context --prompt "$ORIGINAL_PROMPT"
    # o claude tenta terminar; o hook intercepta e reinicia
done
```

**Por que funciona:** o modelo degrada em janelas longas; iterar com contexto fresco + estado no filesystem é mais confiável que tentar fazer uma janela muito longa dar certo. **Trade-off:** custo de tokens sobe (reprocessar o prompt a cada iteração), mas a confiabilidade também.

**Anti-padrão:** tentar microserviços de agentes não-determinísticos — Huntley chama isso de "red hot mess". Mantenha um único processo orquestrador.

### 4.4 Subagentes paralelos (Codex, Mar 2026)

Codex introduziu **subagents** com perfis em TOML, cada um com modelo próprio:

```toml
# ~/.codex/agents/explorer.toml
[agent]
name = "explorer"
model = "gpt-5.4-mini"
tools = ["read_file", "grep", "fs_browser"]

[agent]
name = "implementer"
model = "gpt-5.4"
tools = ["read_file", "write_file", "bash"]
```

**Por que paralelizar:** isolar contexto por subagente evita que ruído de uma subtarefa contamine a principal. O agregador recebe só o resultado final, não os 10k tokens de tool calls.

**Limitação:** subagentes não-determinísticos se comunicando é mais frágil que um único loop bem orquestrado. Comece com um, expanda quando a dor aparecer.

---

## 5. Context engineering — o sangue do harness

### 5.1 AGENTS.md como sumário, não enciclopédia (OpenAI, Feb 2026)

A OpenAI testou o "AGENTS.md gigante de 1000 linhas" e falhou em quatro modos previsíveis:
- Contexto lota, o modelo ignora regras
- Tudo vira "importante" = nada é
- Drifts de regras obsoletas que ninguém mantém
- Sem cobertura mecânica de validade

**Solução da OpenAI:**

```text
AGENTS.md                  ← ~100 linhas, índice/TOC
ARCHITECTURE.md            ← mapa de domínios e camadas
docs/
  ├── design-docs/         ← princípios, core-beliefs
  ├── exec-plans/          ← active/, completed/, tech-debt
  ├── generated/           ← schemas, código-derivado
  ├── product-specs/       ← user stories, features
  └── references/          ← llms.txt de libs externas
DESIGN.md FRONTEND.md PLANS.md PRODUCT_SENSE.md QUALITY_SCORE.md
```

**Princípio:** *progressive disclosure*. O agente começa com 100 linhas estáveis, é ensinado onde olhar em seguida. **Doc-gardening agents** varreram docs obsoletas em PRs automáticos. **Linters customizados** validam que a knowledge base está atual, cross-linked e estruturada.

### 5.2 Context rot e gerenciamento de janela

A **Context Rot** (Chroma Research) descreve como modelos pioram conforme a janela enche. Estratégias de harness:

| Técnica | Quando | Trade-off |
|---|---|---|
| **Compaction** | Input + buffer ≥ context_window | Preserva continuidade; pode preservar context anxiety |
| **Context resets** | Sessão nova, com handoff estruturado | Clean slate; custo de orquestração e latência |
| **Tool output offloading** | Output de ferramenta > threshold tokens | Mantém head/tail no contexto, full output no filesystem |
| **Skills (progressive disclosure)** | Muitas tools/MCPs no start | Metadata carregada sob demanda |
| **Recitação de objetivos** (todo.md) | A cada ~50 tool calls | Mantém metas no fim do attention span |
| **Preservação de error traces** | Após falha | Modelo evita repetir erros |

**Context resets vs. compaction** (Anthropic): compaction não dá clean slate, e modelos como Sonnet 4.5 tinham "context anxiety" que levava a wrap-up prematuro. Opus 4.5 removeu o problema nativamente — exemplo de **harness feature que migra para o modelo** ao longo das gerações.

### 5.3 KV-cache hit rate (Manus, OpenAI)

Para agents com **prefill-to-decode ratio de ~100:1**, hit rate de KV-cache é a métrica de produção mais importante. Tokens cached custam 10x menos (Claude Sonnet). Implicações:

- **Não modifique tool definitions dinamicamente** — invalida cache
- **Use logit masking** em vez de remover tools
- **Mantenha prefixo de prompt estável** entre turns
- **Cache breakpoints** em pontos lógicos (após system prompt, após tools, antes de input do usuário)

---

## 6. Tool design

### 6.1 Menos é mais (Vercel paradox)

Vercel construiu um coding agent, removeu 80% das tools, e o resultado melhorou. Por quê:
- Modelo gasta menos tokens decidindo qual tool chamar
- Menos tool calls erradas
- Argumentos mais consistentes
- Foco em tools com **alta utilidade marginal**

**Regra prática:** uma tool por objetivo principal, descrições claras, retornos estruturados.

### 6.2 Mask tools, don't remove them (Manus)

Manus descobriu que modificar a lista de tools entre turns:
- Invalida o KV-cache
- Confunde o modelo sobre ações anteriores
- Causa erros de "tool not found" em chains longas

**Solução:** mantenha tools registradas, mas use **logit masking** (modifica a distribuição de probabilidade do próximo token) para impedir seleção dinâmica. State machines controlam a transição entre estados onde cada tool é permitida.

### 6.3 Tool design é agent UX (Anthropic)

Princípios:
- **Nome legível** — o modelo lê o nome da tool, então nomeie pelo que ela faz
- **Schema estrito** — campos opcionais minados
- **Error messages projetadas para LLM** — inclua remediation hints
- **Idempotência** — tool chamada duas vezes tem o mesmo efeito
- **Token budget do output** declarado e enforced
- **Princípio de menor privilégio** — exposição mínima de superfície

Exemplos práticos:
- `bash` é a general-purpose tool por default
- `apply_patch` (Codex) é especializado mas tem prompting guide próprio
- Linters customizados com mensagens de erro que **injetam remediation instructions no agent context** (OpenAI)

### 6.4 Ferramentas compound: bash + filesystem + browser

A tríade dominante em 2026:
1. **Bash** — execução geral, agent projeta suas próprias tools via código
2. **Filesystem** — durable state, collaboration surface, offload de contexto
3. **Browser automation** (Playwright MCP) — verificação end-to-end de UI

---

## 7. Verificação — o divisor de águas

### 7.1 Self-evaluation falha consistentemente

Quando um agente avalia o próprio trabalho:
- Skew positivo — LLMs são generosos com outputs de LLMs
- Falha pior em tarefas subjetivas (design) onde não há teste binário
- Mesmo em tarefas verificáveis, julgamento pode ser fraco

**Separação generator/evaluator** (Anthropic GAN-inspired) é a alavanca mais forte. Tunar um evaluator externo para ser cético é tratável; tunar um generator para criticar a si mesmo não é.

### 7.2 Critérios como artefatos versionados

Defina critérios concretos que o evaluator aplica a cada iteração. Para design, Anthropic usa:
- Design quality
- Originality
- Craft
- Functionality

Cada um com **threshold hard** abaixo do qual o sprint falha.

### 7.3 Browser-based verification (Playwright MCP)

Para apps web, **navegue o app real**:
- Click flows
- Screenshots
- DOM snapshots
- Network requests
- Console logs

O evaluator não vê um screenshot estático — ele **interage** com a página antes de avaliar. Runs chegam a 4h só de iteração.

### 7.4 Verificação computacional vs inferencial (Böckeler)

| Tipo | Latência | Custo | Confiabilidade | Onde usar |
|---|---|---|---|---|
| **Computational** (linters, type checkers, structural tests) | ms-s | baixo | determinística | Cada commit, pre-commit, CI |
| **Inferential** (LLM-as-judge, mutation testing) | min | alto | probabilística | Post-merge, code review, drift detection |

**Keep quality left** — checks mais baratos e rápidos o mais cedo possível no lifecycle.

### 7.5 Aritmética da confiabilidade

Cada step com 95% de sucesso encadeado 20 vezes cai para 36%. Para retornar a ≥95% end-to-end, **verification loops** com retry policies e checkpoint-resume são obrigatórios. Casos reportados:
- Vercel: 83% → 96% com verification layer estruturada
- Microsoft Azure SRE: 45% → 75% "Intent Met" migrando de tools bespoke para filesystem-based context engineering
- Meta REA: autônomo em pipelines ML de múltiplos dias com hibernate-and-wake checkpoints

---

## 8. Conhecimento do repositório

### 8.1 Legibilidade do agente é o objetivo (OpenAI)

> "Anything the agent can't access in-context while running effectively doesn't exist."

Implicações:
- Decisões em Slack, Google Docs, knowledge em cabeças de pessoas — **não existem** para o agente
- Prefira dependências "boring" que o modelo consegue modelar
- Em alguns casos é mais barato reimplementar do que lidar com bibliotecas opacas
- Slack threads que decidem arquitetura precisam virar ADRs no repo

### 8.2 Linters customizados como injeção de prompt

Linters de produção devem ter **error messages que incluem remediation instructions para o agente**:

```rust
// error[agent-required-tests]: function lacks test coverage
// help: Add a test case that covers the happy path AND at least one
//       error path. Use the existing `tests/` fixtures and run
//       `cargo test <name>` to verify before committing.
```

Esse é um **feedback sensor** que é deterministic, fast e agent-friendly.

### 8.3 Architecture fitness via constraints mecânicas

OpenAI define camadas (Types → Config → Repo → Service → Runtime → UI) com direção de dependência estrita. Cross-cutting concerns entram por uma única interface explícita: **Providers**. Linters estruturais e testes arquiteturais enforçam essas regras.

```text
[App Settings]
  → Types
  → Config
  → Repo
  → Service
  → Runtime
  → UI
  ↑ Providers (auth, telemetry, feature flags)
```

**Resultado:** agentes podem ser rápidos sem decay ou drift arquitetural. Constraints aplicadas uma vez funcionam em qualquer commit.

---

## 9. Comparação dos principais harnesses

| Harness | Loop | Context | Tools | Subagentes | Sandbox | Differentiator |
|---|---|---|---|---|---|---|
| **Claude Code** | Initializer + Coding | Compaction + resets, claude-progress.txt | MCP + skills | Agent Teams via MCP | Permissions + hooks | Frontend design skill, long-running harness |
| **Codex CLI** | Responses API loop | KV-cache optimized, machine-readable artifacts | TOML agents, MCP | Subagents com modelo próprio (Mar 2026) | Sandbox CLI | Subagents, model-specific instructions |
| **Cursor Composer** | ReAct por modelo | Indexação do repo + reasoning trace | Tools renomeadas por modelo | Subagentes paralelos | Sandbox FS/network | Composer treinado em tool-use trajectories; **30% drop** se reasoning trace perdido |
| **Manus** | ReAct | Filesystem-as-context, todo.md recitation | 75+ providers, logit masking | Sub-agents para isolamento | Sandboxed env | KV-cache hit rate como KPI #1 |
| **OpenCode** | Server-client + LSP | LSP para feedback imediato | 75+ providers | Subagents primário + workers | Approval gates | Provider-agnostic, LSP nativo |
| **Aider** | Repo map + edits | Repo map estático | Edit, bash, search | Não | Aprovações | Repo map, sem agent mode |
| **Cline/Roo Code** | VS Code | Auto-compaction | Tools VS Code | Multi-mode | Sandbox opcional | Mode switching (Architect/Code/Ask) |

---

## 10. Lições práticas para construir seu próprio harness

### 10.1 O que sempre fazer

1. **Comece com augmented LLM** (Anthropic "Building Effective Agents"). LLM + retrieval + tools + memory resolve 80% dos casos.
2. **Adote um Agent Loop canônico** e diferencie-se no gerenciamento de cada passo.
3. **Filesystem é a primitive mais importante** — durable state + collaboration surface.
4. **Compaction + context resets** juntos, não um ou outro.
5. **Verification sempre que possível** — linters, testes, browser automation.
6. **Knowledge base como sistema de record** versionado, com AGENTS.md como TOC de ~100 linhas.
7. **Recite objetivos** no fim do contexto (todo.md) a cada ~50 tool calls.
8. **Preserve error traces** — limpar faz o modelo repetir erros.
9. **Doc-gardening agents** para combater entropia documental.
10. **Custom linters** cujas mensagens injetam remediation hints no agent context.

### 10.2 O que evitar

1. **AGENTS.md gigante** — vira túmulo de regras obsoletas.
2. **Tool sprawl** — Vercel perdeu qualidade adicionando tools, ganhou removendo 80%.
3. **Modificar tool list dinamicamente** — invalida KV-cache e confunde o modelo.
4. **Self-evaluation em tarefas subjetivas** — sempre separe generator e evaluator.
5. **Microserviços de agentes não-determinísticos** — Huntley: "red hot mess".
6. **Múltiplos agents quando um loop bem desenhado resolve** — complexidade de orquestração raramente compensa.
7. **Confiar em testes gerados pelo agente** — Böckeler: ainda não confiável o suficiente; combine com approved fixtures e mutation testing.
8. **Logar secrets, full prompts, full tool output** — vetado em qualquer harness de produção.
9. **Recompactar o mesmo erro** — preserve error traces; corrija o harness, não apenas o output.
10. **Ferramentas que expõem mais do que precisam** — least privilege, idempotência, error surfaces projetadas para LLM.

### 10.3 Quando adicionar complexidade

| Sinal | Resposta |
|---|---|
| Tarefas que cruzam múltiplas context windows | Initializer + coding agents + feature list |
| Avaliação subjetiva repetidamente falha | Generator + evaluator (GAN-inspired) |
| Custo de tokens sobe sem ganho de qualidade | Subagentes com contexto isolado |
| Drift arquitetural recorrente | Linters estruturais com mensagens para LLM |
| Documentação fica obsoleta | Doc-gardening agent + CI de freshness |
| Agente entra em loops de erro | Browser-based verification + retry policies |

---

## 11. Tendências e o futuro

1. **Harness features migram para o modelo.** Cada geração absorve uma camada: context anxiety (Sonnet 4.5 → Opus 4.5), planning, self-verification parcial. Mas prompt engineering e harness engineering continuam valiosos (analogia LangChain).

2. **Harnessability como critério de arquitetura.** Böckeler argumenta que "harnessability" deve ser first-class na escolha de tecnologia — linguagens fortemente tipadas dão type-check como sensor grátis, frameworks com boundaries claras dão architectural fitness. Linguagem de greenfield deve priorizar harnessability.

3. **Co-evolução modelo × harness cria lock-in.** Modelos pós-treinados com harness específico overfittem àquela interface (exemplo: apply_patch no Codex). Trocar tool logic degrada performance. Implicação: harnesses concorrentes vão divergir, não convergir.

4. **Verification assíncrona e contínua.** Drift sensors rodando fora do change lifecycle, AI judges continuamente amostrando qualidade, runtime feedback (SLOs degradando) chegando como context.

5. **Múltiplos orchestrators especializados.** Pattern "Agent Teams" emerge como composable building block — cada orchestrator focado em uma classe de problema, coordenando subagentes sob filesystem compartilhado.

6. **Harness evaluation frameworks.** Anthropic "Demystifying Evals for AI Agents" alerta que unit-test-style evals falham para agents. Em 2026 surgem frameworks específicos: trajectory eval, terminal-bench style, harness ablation studies.

7. **Linguistic natural-language harnesses (NLAHs).** Paper de 2026 propõe externalizar lógica de controle do agente como artefatos em linguagem natural, executados por um runtime compartilhado. Torna harness design **estudável, transferível e reprodutível** em vez de enterrado em framework defaults.

---

## 12. Mapa de fontes

### Posts canônicos
- **Anthropic — Effective harnesses for long-running agents** (Nov 2025): https://www.anthropic.com/engineering/effective-harnesses-for-long-running-agents
- **Anthropic — Harness design for long-running application development** (Mar 2026): https://www.anthropic.com/engineering/harness-design-long-running-apps
- **Anthropic — Building Effective Agents**: https://www.anthropic.com/research/building-effective-agents
- **Anthropic — Writing Effective Tools for Agents**: https://www.anthropic.com/engineering/writing-effective-tools-for-agents
- **Anthropic — Beyond Permission Prompts**: https://www.anthropic.com/engineering/beyond-permission-prompts
- **Anthropic — Demystifying Evals for AI Agents**: https://www.anthropic.com/engineering/demystifying-evals-for-ai-agents
- **OpenAI — Harness engineering** (Feb 2026): https://openai.com/index/harness-engineering/
- **OpenAI — Unrolling the Codex agent loop** (Jan 2026): https://openai.com/index/unrolling-the-codex-agent-loop/
- **OpenAI — Codex subagents**: https://developers.openai.com/codex/subagents
- **LangChain — The Anatomy of an Agent Harness** (Mar 2026): https://www.langchain.com/blog/the-anatomy-of-an-agent-harness
- **Martin Fowler — Harness engineering for coding agent users** (Apr 2026): https://martinfowler.com/articles/harness-engineering.html
- **Morph — Agent Engineering: IMPACT framework** (Mar 2026): https://www.morphllm.com/agent-engineering
- **Manus — Context Engineering for AI Agents**: https://manus.im/blog/Context-Engineering-for-AI-Agents-Lessons-from-Building-Manus
- **Geoffrey Huntley — Ralph Loop**: https://ghuntley.com/loop/
- **Cursor — Codex model harness**: https://cursor.com/blog/codex-model-harness
- **Harness Engineering — Complete Guide**: https://harness-engineering.ai/blog/agent-harness-complete-guide/
- **Awesome Harness Engineering**: https://github.com/ai-boost/awesome-harness-engineering

### Papers
- **Agent Harness for LLM Agents: A Survey** (preprints.org, Apr 2026): https://www.preprints.org/manuscript/202604.0428
- **Building AI Coding Agents for the Terminal** (arxiv 2603.05344)
- **Natural-Language Agent Harnesses** (arxiv 2603.25723)
- **Meta REA — Ads Ranking autonomous system** (Mar 2026): https://engineering.fb.com/2026/03/17/developer-tools/ranking-engineer-agent-rea-autonomous-ai-system-accelerating-meta-ads-ranking-innovation/
- **Microsoft — Azure SRE Agent**: https://techcommunity.microsoft.com/blog/appsonazureblog/how-we-build-azure-sre-agent-with-agentic-workflows/4508753

### Comparativos 2026
- thoughts.jock.pl: Claude Code vs Codex CLI vs Aider vs OpenCode vs Pi vs Cursor
- requesty.ai: Agentic Coding Tools Compared 2026
- youngju.dev: 2026 AI Coding Agent Head-to-Head
- callsphere.ai: Autonomous Coding Agents 2026
- firecrawl.dev: Best AI Coding Agents 2026

---

*Relatório gerado em junho de 2026 a partir de pesquisa web. Cada técnica foi verificada em pelo menos duas fontes independentes antes de ser incluída como recomendação.*
