#!/usr/bin/env node
// ============================================================================
// cordis_demo.mjs - prova executavel dos conceitos do paper
// "A Programming Paradigm for Spatiotemporal Composability" (Shi, Zhang, Cui).
//
// Rode:
//   node cordis_demo.mjs --demo    -> sequencia automatica, depois sai
//   node cordis_demo.mjs           -> REPL interativo (digite help)
//
// Isto NAO e hot-reload por file-watcher (bun dev / vite HMR). Nada e
// recompilado e nenhum processo e reiniciado: o MESMO processo mantem seu
// estado (session.tick) e muta o proprio grafo de componentes atraves de
// efeitos revertiveis + coefeitos reativos (Secoes 3 e 4 do paper).
//
// Mapeamento (paper -> este arquivo):
//   contexto Γ∞               -> Context (store Σ + fibras + renderers)
//   efeito revertivel         -> ctx.effect(fn): fn devolve seu INVERSO;
//                               inversos compoem um acumulador LIFO por fibra
//   especificacao d (coefeito)-> component.inject
//   provisao p               -> component.provide
//   componente/fibra         -> Fiber: INACTIVE/LOADING/ACTIVE/UNLOADING
//   notify / ativacao        -> ctx.notify -> fiber.refresh
//   withdraw guard (4.3.1)   -> Fiber.unload drena dependentes ANTES de rodar
//                               o proprio inverso
// ============================================================================

import { createInterface } from 'node:readline';

const sleep = (ms) => new Promise((r) => setTimeout(r, ms));

// ---------------------------------------------------------------------------
// Context: reifica o Γ∞ do paper. Guarda o store de coefeitos (Σ), o registro
// de fibras, e roteia inversos para o acumulador do dono corrente.
// ---------------------------------------------------------------------------
class Context {
  constructor() {
    this.store = new Map();    // Σ: key -> { value, provider }
    this.fibers = new Map();   // registro: nome -> Fiber
    this.renderers = [];       // a "interface": funcoes registradas via efeitos
    this.owner = null;         // fibra em aplicacao (roteia o acumulador)
    this.logs = [];
    this.dispose = () => {};   // acumulador da raiz
  }
  log(msg) { this.logs.push(msg); }

  // efeito revertivel (Secao 3.1): fn executa o efeito e DEVOLVE o inverso.
  // O inverso entra na pilha LIFO do dono corrente (ou da raiz).
  effect(fn) {
    const inverse = fn();
    const holder = this.owner || this;
    const prev = holder.dispose;
    holder.dispose = () => { inverse(); prev(); };   // LIFO
    return inverse;
  }

  // provisao de coefeito (Secao 3.2): set(k,v) e ele mesmo um efeito.
  provide(key, value) {
    const owner = this.owner;
    return this.effect(() => {
      if (this.store.has(key)) throw new Error('chave ja provida: ' + key);
      this.store.set(key, { value, provider: owner });
      this.log('+ prove ' + key);
      this.notify([key]);
      return () => {
        this.store.delete(key);
        this.log('- retira ' + key);
        this.notify([key]);
      };
    });
  }

  get(key) { const e = this.store.get(key); return e ? e.value : undefined; }

  // Resolucao de dependencias: uma chave so conta se o provedor esta ACTIVE
  // (σ_γ une apenas tabelas de fibras ACTIVE).
  resolve(inject) {
    const view = {};
    for (const k of inject) {
      const e = this.store.get(k);
      if (!e || e.provider.state !== 'ACTIVE') return null;
      view[k] = e.value;
    }
    return view;
  }

  // instanciacao de componente (Definicao 47). Registro = efeito rastreado;
  // seu inverso = aposentar (O-Retire).
  use(component, config = {}) {
    const name = component.name;
    if (this.fibers.has(name)) throw new Error('fibra ja existe: ' + name);
    const fiber = new Fiber(this, component, config);
    this.fibers.set(name, fiber);
    this.log('O-Insert ' + name);
    this.effect(() => {
      fiber.refresh();
      return () => { fiber.retired = true; fiber.refresh(); this.log('O-Retire ' + name); };
    });
    return fiber;
  }

  retire(name) {
    const f = this.fibers.get(name);
    if (!f) throw new Error('sem fibra: ' + name);
    if (!f.retired) { f.retired = true; this.log('O-Retire ' + name); }
    f.refresh();
  }

  remove(name) {
    const f = this.fibers.get(name);
    if (f && f.state === 'INACTIVE') {
      this.fibers.delete(name);
      this.log('O-Remove ' + name);
    } else if (f) {
      throw new Error('nao posso remover ' + name + ': estado ' + f.state);
    }
  }

  // notificacao reativa (Secao 3.2.2)
  notify(keys) {
    for (const f of this.fibers.values()) {
      if (f.inject.some((k) => keys.includes(k))) f.refresh();
    }
  }
}

// ---------------------------------------------------------------------------
// Fiber: instancia de um componente, com ciclo de vida proprio.
// ---------------------------------------------------------------------------
class Fiber {
  constructor(ctx, component, config) {
    this.ctx = ctx;
    this.component = component;
    this.config = config;
    this.name = component.name;
    this.inject = component.inject || [];    // d: especificacao de coefeitos
    this.provide = component.provide || [];  // p: provisao
    this.state = 'INACTIVE';                 // θ
    this.dispose = () => {};                 // acumulador g (inversos LIFO)
    this.committed = null;                   // ω: visao contra a qual ativou
    this.retired = false;
  }

  targetView() {
    if (this.retired) return null;
    return this.ctx.resolve(this.inject);
  }

  refresh() {
    const t = this.targetView();
    if (t === null && this.state === 'ACTIVE') {
      this.state = 'UNLOADING';   // L-Leave: para de prover, drena dependentes
      this.unload();
    } else if (t !== null && this.state === 'INACTIVE') {
      this.state = 'LOADING';     // L-Begin
      this.reload(t);
    }
  }

  reload(view) {
    this.committed = view;
    this.ctx.owner = this;
    this.component.apply(this.ctx, this.config);  // efeitos caem em this.dispose
    this.ctx.owner = null;
    this.state = 'ACTIVE';
    this.ctx.log('>> ativou ' + this.name);
    this.ctx.notify(this.provide);
  }

  unload() {
    // withdraw guard (Secao 4.3.1): dependentes desativam PRIMEIRO, enquanto
    // nosso valor ainda esta legivel.
    for (const f of this.ctx.fibers.values()) {
      if (f !== this && f.state === 'ACTIVE' &&
          f.inject.some((k) => this.provide.includes(k))) {
        f.refresh();
      }
    }
    this.dispose();             // roda os inversos acumulados (LIFO)
    this.dispose = () => {};
    this.committed = null;
    this.state = 'INACTIVE';
    this.ctx.log('<< desativou ' + this.name + ' (efeitos revertidos)');
    this.ctx.notify(this.provide);
  }
}

// ============================ componentes ============================

// Provedor: contribui um coefeito (title), nada visual por si.
const Header = {
  name: 'Header',
  provide: ['title'],
  inject: [],
  apply(ctx, cfg) { ctx.provide('title', cfg.text); },
};

// Consumidor: renderiza apenas enquanto a chave 'title' esta provida (reativo).
const TitleRenderer = {
  name: 'TitleRenderer',
  provide: [],
  inject: ['title'],
  apply(ctx) {
    const title = ctx.get('title');
    ctx.effect(() => {
      const fn = () => title;
      ctx.renderers.push(fn);
      ctx.log('+ TitleRenderer registra uma linha de render');
      return () => {
        const i = ctx.renderers.indexOf(fn);
        if (i >= 0) ctx.renderers.splice(i, 1);
        ctx.log('- TitleRenderer reverte sua linha (title ainda legivel: "' + ctx.get('title') + '")');
      };
    });
  },
};

// Efeito puro: registra a linha de tick, nao prove nem depende de nada.
const Clock = {
  name: 'Clock',
  provide: [],
  inject: [],
  apply(ctx) {
    ctx.effect(() => {
      const fn = (s) => 'tick = ' + s.tick;
      ctx.renderers.push(fn);
      ctx.log('+ Clock registra linha de tick');
      return () => {
        const i = ctx.renderers.indexOf(fn);
        if (i >= 0) ctx.renderers.splice(i, 1);
        ctx.log('- Clock reverte linha de tick');
      };
    });
  },
};

const Footer = {
  name: 'Footer',
  provide: ['footer'],
  inject: [],
  apply(ctx, cfg) { ctx.provide('footer', cfg.text); },
};

const FooterRenderer = {
  name: 'FooterRenderer',
  provide: [],
  inject: ['footer'],
  apply(ctx) {
    const footer = ctx.get('footer');
    ctx.effect(() => {
      const fn = () => footer;
      ctx.renderers.push(fn);
      ctx.log('+ FooterRenderer registra uma linha');
      return () => {
        const i = ctx.renderers.indexOf(fn);
        if (i >= 0) ctx.renderers.splice(i, 1);
        ctx.log('- FooterRenderer reverte sua linha');
      };
    });
  },
};

// ============================== render ==============================

const BORDER = '-'.repeat(48);
function render(ctx, session) {
  const out = [];
  out.push('+' + BORDER + '+');
  const row = (s) => out.push('| ' + String(s).padEnd(46) + ' |');
  for (const fn of ctx.renderers) row(fn(session));
  row('');
  row('session.tick = ' + session.tick + '   <- estado persistente (sobrevive aos swaps)');
  row('');
  row('fibers:');
  for (const f of ctx.fibers.values()) {
    let s = f.name + ' [' + f.state + ']';
    if (f.inject.length) s += '  injects:' + f.inject.join(',');
    if (f.provide.length) s += '  provides:' + f.provide.join(',');
    row(s);
  }
  out.push('+' + BORDER + '+');
  return out.join('\n');
}

function drain(ctx) {
  for (const l of ctx.logs) console.log('    - ' + l);
  ctx.logs.length = 0;
}
function say(msg) { console.log(''); console.log('### ' + msg); }
function show(ctx, session, n) {
  console.log('');
  console.log('--- frame ' + n + ' (tick ' + session.tick + ') ---');
  console.log(render(ctx, session));
}

function unload(ctx, name) { ctx.retire(name); }
function load(ctx, component, config = {}) {
  const name = component.name;
  if (ctx.fibers.has(name)) {
    const f = ctx.fibers.get(name);
    if (f.state !== 'INACTIVE') ctx.retire(name);
    ctx.remove(name);
  }
  ctx.use(component, config);
}

// ============================= demo scriptada ============================

async function demo() {
  const ctx = new Context();
  const session = { tick: 0 };
  let n = 0;

  say('Boot: carregando componentes (tudo via efeitos rastreados)');
  ctx.use(Header, { text: '=== Cordis Demo v1 ===' });
  ctx.use(TitleRenderer);
  ctx.use(Clock);
  ctx.use(Footer, { text: 'rodape: rodando' });
  ctx.use(FooterRenderer);
  drain(ctx);
  show(ctx, session, ++n);

  for (let i = 0; i < 3; i++) { session.tick++; await sleep(500); show(ctx, session, ++n); }

  say('DESCARREGAR Header (provedor de "title") - observe a cascata reativa');
  unload(ctx, 'Header');
  drain(ctx);
  show(ctx, session, ++n);

  for (let i = 0; i < 2; i++) { session.tick++; await sleep(500); show(ctx, session, ++n); }

  say('RECARREGAR Header v2 no mesmo slot (conteudo novo, sem reiniciar)');
  load(ctx, Header, { text: '=== Cordis Demo v2 (HOT-SWAP) ===' });
  drain(ctx);
  show(ctx, session, ++n);

  for (let i = 0; i < 2; i++) { session.tick++; await sleep(500); show(ctx, session, ++n); }

  say('DESCARREGAR Clock - a linha some, mas session.tick continua');
  unload(ctx, 'Clock');
  drain(ctx);
  show(ctx, session, ++n);

  for (let i = 0; i < 2; i++) { session.tick++; await sleep(500); show(ctx, session, ++n); }

  say('RECARREGAR Clock - tick NAO zerou: nenhum restart aconteceu');
  load(ctx, Clock);
  drain(ctx);
  show(ctx, session, ++n);

  console.log('');
  console.log('Fim. session.tick atravessou 2 hot-swaps sem reset (=' + session.tick + ').');
}

// ============================ REPL interativo ============================

function interactive() {
  const ctx = new Context();
  const session = { tick: 0 };
  let n = 0;

  ctx.use(Header, { text: '=== Cordis Demo v1 ===' });
  ctx.use(TitleRenderer);
  ctx.use(Clock);
  ctx.use(Footer, { text: 'rodape: rodando' });
  ctx.use(FooterRenderer);
  drain(ctx);
  show(ctx, session, ++n);

  const timer = setInterval(() => { session.tick++; show(ctx, session, ++n); }, 1200);

  const help = () => {
    console.log('Comandos:');
    console.log('  header <texto>   troca o provedor Header para <texto>');
    console.log('  off header       descarrega Header (cascata reativa)');
    console.log('  on header        recarrega Header');
    console.log('  off clock        descarrega Clock');
    console.log('  on clock         recarrega Clock');
    console.log('  quit             sai');
  };
  help();

  const rl = createInterface({ input: process.stdin, output: process.stdout });
  rl.setPrompt('> ');
  rl.prompt();
  rl.on('line', (line) => {
    const parts = line.trim().split(/ +/);
    const cmd = parts[0];
    const arg = parts.slice(1).join(' ');
    try {
      if (cmd === 'header') {
        load(ctx, Header, { text: arg || '=== novo ===' });
      } else if (cmd === 'off' && arg === 'header') {
        unload(ctx, 'Header');
      } else if (cmd === 'on' && arg === 'header') {
        load(ctx, Header, { text: '=== Cordis Demo v1 ===' });
      } else if (cmd === 'off' && arg === 'clock') {
        unload(ctx, 'Clock');
      } else if (cmd === 'on' && arg === 'clock') {
        load(ctx, Clock);
      } else if (cmd === 'quit' || cmd === 'exit') {
        clearInterval(timer); rl.close(); process.exit(0);
      } else {
        help(); rl.prompt(); return;
      }
      drain(ctx);
      show(ctx, session, ++n);
    } catch (e) {
      console.log('erro: ' + e.message);
    }
    rl.prompt();
  });
}

if (process.argv.includes('--demo')) {
  await demo();
  process.exit(0);
} else {
  interactive();
}