(function () {
  "use strict";

  const state = {
    runs: [],
    selectedCaseId: null,
  };

  const el = {
    fileInput: document.getElementById("fileInput"),
    clearButton: document.getElementById("clearButton"),
    dropZone: document.getElementById("dropZone"),
    emptyState: document.getElementById("emptyState"),
    dashboard: document.getElementById("dashboard"),
    loadedSummary: document.getElementById("loadedSummary"),
    summaryCards: document.getElementById("summaryCards"),
    overviewCharts: document.getElementById("overviewCharts"),
    benchmarkCharts: document.getElementById("benchmarkCharts"),
    runList: document.getElementById("runList"),
    caseNav: document.getElementById("caseNav"),
    caseMatrix: document.getElementById("caseMatrix"),
    caseDetails: document.getElementById("caseDetails"),
    cardTemplate: document.getElementById("cardTemplate"),
  };

  const chartSpecs = [
    {
      key: "pass",
      title: "Verified Success",
      subtitle: "Higher is better",
      select: (run) => number(run.metrics.verified_success_rate),
      format: percent,
      better: "higher",
      color: "ok",
    },
    {
      key: "tokens",
      title: "Tokens per Success",
      subtitle: "Lower is better",
      select: (run) => nullable(run.metrics.tokens_per_success),
      format: (value) => optionalNumber(value, 0),
      better: "lower",
      color: "warn",
    },
    {
      key: "tools",
      title: "Tool Calls per Success",
      subtitle: "Lower is better",
      select: (run) => nullable(run.metrics.tool_calls_per_success),
      format: (value) => optionalNumber(value, 1),
      better: "lower",
      color: "accent",
    },
    {
      key: "trace",
      title: "Trace Events",
      subtitle: "Trajectory size",
      select: traceEventCount,
      format: (value) => formatNumber(value, 0),
      better: "lower",
      color: "neutral",
    },
    {
      key: "wall",
      title: "Wall Time",
      subtitle: "End-to-end runtime",
      select: (run) => number(run.metrics.wall_time_ms),
      format: duration,
      better: "lower",
      color: "neutral",
    },
    {
      key: "failures",
      title: "Failed Cases",
      subtitle: "Debug first",
      select: (run) => number(run.metrics.failed_cases),
      format: (value) => formatNumber(value, 0),
      better: "lower",
      color: "danger",
    },
  ];

  el.fileInput.addEventListener("change", (event) => {
    loadFiles(Array.from(event.target.files || []));
    el.fileInput.value = "";
  });

  el.clearButton.addEventListener("click", () => {
    state.runs = [];
    state.selectedCaseId = null;
    render();
  });

  ["dragenter", "dragover"].forEach((name) => {
    el.dropZone.addEventListener(name, (event) => {
      event.preventDefault();
      el.dropZone.classList.add("dragging");
    });
  });

  ["dragleave", "drop"].forEach((name) => {
    el.dropZone.addEventListener(name, () => el.dropZone.classList.remove("dragging"));
  });

  el.dropZone.addEventListener("drop", (event) => {
    event.preventDefault();
    loadFiles(Array.from(event.dataTransfer.files || []));
  });

  render(loadIndexedRuns());

  function loadIndexedRuns() {
    const indexedRuns = Array.isArray(window.NAVI_BENCHMARK_RUNS) ? window.NAVI_BENCHMARK_RUNS : [];
    const loaded = [];
    const errors = [];
    for (const entry of indexedRuns) {
      try {
        const run = entry && entry.run ? entry.run : entry;
        validateRun(run, entry?.source_file || "benchmarks/runs/index.js");
        run.source_file = entry?.source_file || run.source_file || "indexed";
        loaded.push(run);
      } catch (error) {
        errors.push(error.message);
      }
    }
    state.runs = dedupeRuns(loaded).sort(compareRuns);
    state.selectedCaseId = pickInitialCase(state.runs);
    return errors;
  }

  async function loadFiles(files) {
    const jsonFiles = files.filter((file) => file.name.endsWith(".json"));
    const loaded = [];
    const errors = [];

    for (const file of jsonFiles) {
      try {
        const text = await file.text();
        const parsed = JSON.parse(text);
        const runs = Array.isArray(parsed) ? parsed : [parsed];
        for (const run of runs) {
          validateRun(run, file.name);
          run.source_file = file.name;
          loaded.push(run);
        }
      } catch (error) {
        errors.push(`${file.name}: ${error.message}`);
      }
    }

    state.runs = dedupeRuns(state.runs.concat(loaded)).sort(compareRuns);
    state.selectedCaseId = state.selectedCaseId || pickInitialCase(state.runs);
    render(errors);
  }

  function validateRun(run, source) {
    if (!run || typeof run !== "object") {
      throw new Error("expected a BenchRun object");
    }
    if (!run.run_id || !run.metrics || !Array.isArray(run.results)) {
      throw new Error(`missing BenchRun fields in ${source}`);
    }
  }

  function dedupeRuns(runs) {
    const map = new Map();
    for (const run of runs) {
      map.set(`${run.run_id}:${run.source_file || ""}`, run);
    }
    return Array.from(map.values());
  }

  function compareRuns(a, b) {
    return number(a.started_at) - number(b.started_at) || String(a.run_id).localeCompare(String(b.run_id));
  }

  function render(errors = []) {
    const hasRuns = state.runs.length > 0;
    el.emptyState.classList.toggle("hidden", hasRuns);
    el.dashboard.classList.toggle("hidden", !hasRuns);
    document.body.classList.toggle("has-data", hasRuns);

    renderSidebar();
    if (!hasRuns) {
      return;
    }

    state.selectedCaseId = state.selectedCaseId || pickInitialCase(state.runs);
    renderSummary(errors);
    renderOverviewCharts();
    renderCharts();
    renderCaseMatrix();
    renderCaseDetails();
  }

  function renderSidebar() {
    el.runList.textContent = "";
    el.caseNav.textContent = "";

    if (!state.runs.length) {
      el.runList.innerHTML = `<span class="empty-nav">Load JSON checkpoints</span>`;
      el.caseNav.innerHTML = `<span class="empty-nav">No cases loaded</span>`;
      return;
    }

    for (const run of state.runs) {
      const item = document.createElement("a");
      item.href = "#breakdown";
      item.className = number(run.metrics.failed_cases) > 0 ? "nav-item fail" : "nav-item pass";
      item.innerHTML = `
        <span>${escapeHtml(shortRun(run))}</span>
        <small>${escapeHtml(benchmarkLabel(run))} · ${percent(run.metrics.verified_success_rate)}</small>
      `;
      el.runList.appendChild(item);
    }

    for (const summary of caseSummaries()) {
      const button = document.createElement("button");
      button.type = "button";
      button.className = `nav-item case-link${summary.caseId === state.selectedCaseId ? " active" : ""}${
        summary.failures ? " fail" : " pass"
      }`;
      button.innerHTML = `
        <span>${escapeHtml(summary.caseId)}</span>
        <small>${summary.passes}/${summary.total} pass · ${summary.failures} fail</small>
      `;
      button.addEventListener("click", () => {
        state.selectedCaseId = summary.caseId;
        renderSidebar();
        renderCaseMatrix();
        renderCaseDetails();
        document.getElementById("trace")?.scrollIntoView({ behavior: "smooth", block: "start" });
      });
      el.caseNav.appendChild(button);
    }
  }

  function renderSummary(errors) {
    const latest = state.runs[state.runs.length - 1];
    const metrics = latest.metrics || {};
    const previous = state.runs.length > 1 ? state.runs[state.runs.length - 2] : null;
    const failingCases = (latest.results || []).filter((result) => !result.passed).length;

    el.loadedSummary.textContent = `${state.runs.length} checkpoint(s), ${allCaseIds().length} case(s), latest ${latest.run_id} on ${benchmarkLabel(latest)}.`;
    el.summaryCards.textContent = "";

    [
      ["Success", percent(metrics.verified_success_rate), previous ? passDelta(previous, latest) : "No baseline"],
      ["Failing cases", failingCases, failingCases ? "Open failures below" : "Latest run clean"],
      ["Tokens/success", optionalNumber(metrics.tokens_per_success, 0), deltaText("tokens_per_success")],
      ["Tools/success", optionalNumber(metrics.tool_calls_per_success, 1), deltaText("tool_calls_per_success")],
      ["Model", compactModelLabel(latest), errors.length ? `${errors.length} load error(s)` : "Benchmark base"],
    ].forEach(([label, value, note]) => {
      const card = el.cardTemplate.content.firstElementChild.cloneNode(true);
      card.querySelector(".metric-label").textContent = label;
      card.querySelector(".metric-value").textContent = value;
      card.querySelector(".metric-note").textContent = note;
      el.summaryCards.appendChild(card);
    });
  }

  function renderOverviewCharts() {
    const latest = state.runs[state.runs.length - 1];
    const selectedCase = findCase(latest, state.selectedCaseId) || latest.results?.[0];
    el.overviewCharts.textContent = "";

    el.overviewCharts.appendChild(
      donutCard({
        title: "Latest Outcome",
        subtitle: latest.run_id,
        segments: [
          { label: "Pass", value: number(latest.metrics.passed_cases), color: "var(--ok)" },
          { label: "Fail", value: number(latest.metrics.failed_cases), color: "var(--danger)" },
        ],
        center: percent(latest.metrics.verified_success_rate),
        foot: `${number(latest.metrics.passed_cases)}/${number(latest.metrics.total_cases)} cases passed`,
      }),
    );

    el.overviewCharts.appendChild(
      stackedRunsCard({
        title: "Checkpoint Outcome Mix",
        subtitle: "Pass/fail distribution per run",
      }),
    );

    el.overviewCharts.appendChild(
      columnCard({
        title: "Case Token Cost",
        subtitle: "Latest checkpoint",
        items: latestCaseMetricItems(latest, (result) => number(result.metrics?.total_tokens)),
        format: (value) => formatNumber(value, 0),
        colorClass: "warn",
      }),
    );

    el.overviewCharts.appendChild(
      columnCard({
        title: "Case Tool Calls",
        subtitle: "Latest checkpoint",
        items: latestCaseMetricItems(latest, (result) => number(result.metrics?.tool_calls)),
        format: (value) => formatNumber(value, 0),
        colorClass: "accent",
      }),
    );

    el.overviewCharts.appendChild(
      donutCard({
        title: "Selected Case Trace",
        subtitle: selectedCase?.case_id || "No case",
        segments: traceSegments(selectedCase?.events || []),
        center: formatNumber((selectedCase?.events || []).length, 0),
        foot: "runtime events captured",
      }),
    );

    el.overviewCharts.appendChild(
      columnCard({
        title: "Run Wall Time",
        subtitle: "Checkpoint runtime",
        items: state.runs.map((run) => ({
          label: shortRun(run),
          value: number(run.metrics.wall_time_ms),
        })),
        format: duration,
        colorClass: "neutral",
      }),
    );
  }

  function renderCharts() {
    el.benchmarkCharts.textContent = "";
    for (const spec of chartSpecs) {
      const values = state.runs.map((run) => spec.select(run));
      const max = Math.max(...values.filter(Number.isFinite).map(Math.abs), 1);
      const chart = document.createElement("article");
      chart.className = "chart-card";
      chart.innerHTML = `
        <div class="chart-head">
          <div>
            <h3>${escapeHtml(spec.title)}</h3>
            <p>${escapeHtml(spec.subtitle)}</p>
          </div>
        </div>
        <div class="bar-list"></div>
      `;
      const list = chart.querySelector(".bar-list");
      state.runs.forEach((run, index) => {
        const value = values[index];
        const width = spec.key === "pass" ? clamp(value, 0, 1) * 100 : (Math.abs(number(value)) / max) * 100;
        const row = document.createElement("div");
        row.className = `bar-row ${spec.color}`;
        row.innerHTML = `
          <span class="bar-label">${escapeHtml(shortRun(run))}</span>
          <span class="bar-track"><span class="bar-fill" style="width:${Math.max(width, 2)}%"></span></span>
          <strong>${escapeHtml(spec.format(value))}</strong>
        `;
        list.appendChild(row);
      });
      el.benchmarkCharts.appendChild(chart);
    }
  }

  function donutCard({ title, subtitle, segments, center, foot }) {
    const total = segments.reduce((sum, segment) => sum + number(segment.value), 0);
    let cursor = 0;
    const stops = segments
      .filter((segment) => number(segment.value) > 0)
      .map((segment) => {
        const start = cursor;
        const end = start + (number(segment.value) / Math.max(total, 1)) * 100;
        cursor = end;
        return `${segment.color} ${start}% ${end}%`;
      })
      .join(", ");
    const card = document.createElement("article");
    card.className = "visual-card donut-card";
    card.innerHTML = `
      <div class="chart-head">
        <div>
          <h3>${escapeHtml(title)}</h3>
          <p>${escapeHtml(subtitle)}</p>
        </div>
      </div>
      <div class="donut-row">
        <div class="donut" style="background: conic-gradient(${stops || "var(--surface-3) 0% 100%"});">
          <span>${escapeHtml(center)}</span>
        </div>
        <div class="donut-legend">
          ${segments
            .map(
              (segment) => `
                <span>
                  <i style="background:${segment.color}"></i>
                  ${escapeHtml(segment.label)}
                  <strong>${formatNumber(segment.value, 0)}</strong>
                </span>
              `,
            )
            .join("")}
        </div>
      </div>
      <p class="chart-foot">${escapeHtml(foot)}</p>
    `;
    return card;
  }

  function stackedRunsCard({ title, subtitle }) {
    const card = document.createElement("article");
    card.className = "visual-card";
    card.innerHTML = `
      <div class="chart-head">
        <div>
          <h3>${escapeHtml(title)}</h3>
          <p>${escapeHtml(subtitle)}</p>
        </div>
      </div>
      <div class="stack-list">
        ${state.runs
          .map((run) => {
            const total = Math.max(number(run.metrics.total_cases), 1);
            const passWidth = (number(run.metrics.passed_cases) / total) * 100;
            const failWidth = (number(run.metrics.failed_cases) / total) * 100;
            return `
              <div class="stack-row">
                <span>${escapeHtml(shortRun(run))}</span>
                <div class="stack-track">
                  <i class="pass" style="width:${passWidth}%"></i>
                  <i class="fail" style="width:${failWidth}%"></i>
                </div>
                <strong>${number(run.metrics.passed_cases)}/${number(run.metrics.total_cases)}</strong>
              </div>
            `;
          })
          .join("")}
      </div>
    `;
    return card;
  }

  function columnCard({ title, subtitle, items, format, colorClass }) {
    const max = Math.max(...items.map((item) => number(item.value)), 1);
    const card = document.createElement("article");
    card.className = "visual-card column-card";
    card.innerHTML = `
      <div class="chart-head">
        <div>
          <h3>${escapeHtml(title)}</h3>
          <p>${escapeHtml(subtitle)}</p>
        </div>
      </div>
      <div class="columns ${escapeHtml(colorClass)}">
        ${items
          .map((item) => {
            const height = Math.max((number(item.value) / max) * 100, 4);
            return `
              <div class="column-item" title="${escapeHtml(item.label)}: ${escapeHtml(format(item.value))}">
                <strong>${escapeHtml(format(item.value))}</strong>
                <span class="column-track"><i style="height:${height}%"></i></span>
                <small>${escapeHtml(compactLabel(item.label))}</small>
              </div>
            `;
          })
          .join("")}
      </div>
    `;
    return card;
  }

  function renderCaseMatrix() {
    const cases = allCaseIds();
    el.caseMatrix.style.setProperty("--run-count", Math.max(state.runs.length, 1));
    el.caseMatrix.textContent = "";

    const header = document.createElement("div");
    header.className = "matrix-row matrix-header";
    header.appendChild(matrixCell("Case"));
    for (const run of state.runs) {
      header.appendChild(matrixCell(shortRun(run)));
    }
    el.caseMatrix.appendChild(header);

    for (const caseId of cases) {
      const row = document.createElement("button");
      row.type = "button";
      row.className = `matrix-row matrix-case${caseId === state.selectedCaseId ? " selected" : ""}`;
      row.addEventListener("click", () => {
        state.selectedCaseId = caseId;
        renderSidebar();
        renderCaseMatrix();
        renderCaseDetails();
      });
      row.appendChild(matrixCell(caseId));
      for (const run of state.runs) {
        const result = findCase(run, caseId);
        const cell = matrixCell(result ? matrixCellText(result) : "-");
        if (result) {
          cell.classList.add(result.passed ? "pass" : "fail");
          cell.title = `${result.title || caseId} · ${formatNumber(result.metrics?.total_tokens, 0)} tokens · ${
            result.metrics?.tool_calls || 0
          } tools`;
        }
        row.appendChild(cell);
      }
      el.caseMatrix.appendChild(row);
    }
  }

  function renderCaseDetails() {
    const caseId = state.selectedCaseId || pickInitialCase(state.runs);
    state.selectedCaseId = caseId;
    if (!caseId) {
      el.caseDetails.innerHTML = "<h2>Case Analysis</h2><p>No cases loaded.</p>";
      return;
    }

    const rows = state.runs
      .map((run) => ({ run, result: findCase(run, caseId) }))
      .filter((entry) => entry.result);
    const latest = rows[rows.length - 1]?.result;
    const latestRun = rows[rows.length - 1]?.run;
    const failures = rows.flatMap(({ run, result }) => caseFailures(run, result));
    const latestEvents = latest?.events || [];

    el.caseDetails.innerHTML = `
      <div id="trace" class="panel-header">
        <div>
          <h2>${escapeHtml(caseId)}</h2>
          <p>${escapeHtml(latest?.title || "Case analysis")}</p>
        </div>
        <div class="detail-actions">
          <button type="button" data-copy="assistant">Copy assistant</button>
          <button type="button" data-copy="case-json">Copy case JSON</button>
        </div>
      </div>

      <div class="analysis-grid">
        <div class="detail-block">
          <h3>Case history</h3>
          ${renderCaseHistoryCards(rows)}
        </div>

        <div class="detail-block">
          <h3>Failures</h3>
          ${renderFailureList(failures)}
        </div>
      </div>

      <div class="analysis-grid lower">
        <div class="detail-block">
          <h3>Trace composition</h3>
          ${renderTraceDonut(latestEvents)}
        </div>

        <div class="detail-block">
          <h3>Trace timeline</h3>
          ${renderTrace(latestEvents)}
        </div>

        <div id="artifacts" class="detail-block">
          <h3>Generated results</h3>
          ${renderArtifacts(latestRun, latest)}
        </div>
      </div>

      <div class="detail-block">
        <h3>Assistant output</h3>
        <pre class="pre">${escapeHtml(latest?.assistant_text || "No assistant output captured.")}</pre>
      </div>
    `;

    el.caseDetails.querySelector('[data-copy="assistant"]')?.addEventListener("click", () => {
      copyText(latest?.assistant_text || "");
    });
    el.caseDetails.querySelector('[data-copy="case-json"]')?.addEventListener("click", () => {
      copyText(JSON.stringify({ run: latestRun, result: latest }, null, 2));
    });
  }

  function renderTrace(events) {
    if (!events.length) {
      return "<p>No runtime events captured for this case.</p>";
    }
    const interesting = events.filter((event) => {
      const kind = eventKindName(event);
      return (
        kind.includes("Tool") ||
        kind.includes("Approval") ||
        kind.includes("Harness") ||
        kind.includes("Tokens") ||
        kind.includes("Error") ||
        kind.includes("Turn")
      );
    });
    const rows = (interesting.length ? interesting : events).slice(0, 120);
    return `
      <ol class="trace-list">
        ${rows
          .map((event, index) => {
            const kind = eventKindName(event);
            return `<li class="${traceClass(kind)}">
              <span>${index + 1}</span>
              <strong>${escapeHtml(kind)}</strong>
              <code>${escapeHtml(eventSummary(event))}</code>
            </li>`;
          })
          .join("")}
      </ol>
    `;
  }

  function renderTraceDonut(events) {
    const segments = traceSegments(events);
    const total = events.length;
    return donutCard({
      title: "Trace composition",
      subtitle: `${formatNumber(total, 0)} event(s)`,
      segments,
      center: formatNumber(total, 0),
      foot: "tool, token, harness, and lifecycle events",
    }).innerHTML;
  }

  function renderArtifacts(run, result) {
    if (!result) {
      return "<p>No selected result.</p>";
    }
    const verifierFailures = caseFailures(run, result);
    return `
      <dl class="artifact-list">
        <dt>Source JSON</dt>
        <dd>${escapeHtml(run?.source_file || "loaded checkpoint")}</dd>
        <dt>Workspace</dt>
        <dd><code>${escapeHtml(result.workspace || "temporary workspace removed")}</code></dd>
        <dt>Verifier failures</dt>
        <dd>${verifierFailures.length}</dd>
        <dt>Events</dt>
        <dd>${formatNumber((result.events || []).length, 0)}</dd>
        <dt>Assistant output</dt>
        <dd>${formatNumber((result.assistant_text || "").length, 0)} chars</dd>
      </dl>
      <pre class="pre small">${escapeHtml(JSON.stringify(result.verifier_results || [], null, 2))}</pre>
    `;
  }

  function caseSummaries() {
    return allCaseIds()
      .map((caseId) => {
        const results = state.runs.map((run) => findCase(run, caseId)).filter(Boolean);
        const passes = results.filter((result) => result.passed).length;
        return { caseId, total: results.length, passes, failures: results.length - passes };
      })
      .sort((a, b) => b.failures - a.failures || a.caseId.localeCompare(b.caseId));
  }

  function allCaseIds() {
    const ids = new Set();
    for (const run of state.runs) {
      for (const result of run.results || []) {
        ids.add(result.case_id);
      }
    }
    return Array.from(ids).sort();
  }

  function findCase(run, caseId) {
    return (run.results || []).find((result) => result.case_id === caseId);
  }

  function pickInitialCase(runs) {
    for (let index = runs.length - 1; index >= 0; index -= 1) {
      const failed = (runs[index].results || []).find((result) => !result.passed);
      if (failed) {
        return failed.case_id;
      }
    }
    return runs[0]?.results?.[0]?.case_id || null;
  }

  function caseTimelineRow(run, result) {
    const metrics = result.metrics || {};
    const diff = `+${number(metrics.diff_lines_added)} / -${number(metrics.diff_lines_removed)}`;
    return `
      <tr>
        <td>${escapeHtml(shortRun(run))}</td>
        <td>${statusPill(result.passed, result.passed ? "PASS" : "FAIL")}</td>
        <td>${formatNumber(metrics.total_tokens, 0)}</td>
        <td>${formatNumber(metrics.tool_calls, 0)}</td>
        <td>${formatNumber(metrics.failed_tool_calls, 0)}</td>
        <td>${formatNumber(metrics.files_changed, 0)}</td>
        <td>${diff}</td>
        <td>${duration(metrics.wall_time_ms)}</td>
      </tr>
    `;
  }

  function renderCaseHistoryCards(rows) {
    return `
      <div class="case-history-grid">
        ${rows
          .map(({ run, result }) => {
            const metrics = result.metrics || {};
            const diff = `+${number(metrics.diff_lines_added)} / -${number(metrics.diff_lines_removed)}`;
            return `
              <article class="case-history-card ${result.passed ? "pass" : "fail"}">
                <header>
                  <strong>${escapeHtml(shortRun(run))}</strong>
                  ${statusPill(result.passed, result.passed ? "PASS" : "FAIL")}
                </header>
                <div class="mini-metrics">
                  <span><b>${formatNumber(metrics.total_tokens, 0)}</b><small>tokens</small></span>
                  <span><b>${formatNumber(metrics.tool_calls, 0)}</b><small>tools</small></span>
                  <span><b>${formatNumber(metrics.failed_tool_calls, 0)}</b><small>tool fail</small></span>
                  <span><b>${duration(metrics.wall_time_ms)}</b><small>time</small></span>
                </div>
                <footer>${formatNumber(metrics.files_changed, 0)} files · ${diff} lines</footer>
              </article>
            `;
          })
          .join("")}
      </div>
    `;
  }

  function caseFailures(run, result) {
    const failures = [];
    if (result.error) {
      failures.push({ run, text: result.error });
    }
    for (const verifier of (result.setup_results || []).concat(result.verifier_results || [])) {
      if (!verifierOk(verifier)) {
        const code = verifier.exit_code == null ? "" : ` (${verifier.exit_code})`;
        failures.push({
          run,
          text: `${verifier.command || "verifier"} -> ${verifier.status}${code}`,
        });
      }
    }
    return failures;
  }

  function renderFailureList(failures) {
    if (!failures.length) {
      return "<p>No verifier or agent failures for this case.</p>";
    }
    return `
      <ul class="failure-list">
        ${failures
          .map(
            (failure) =>
              `<li><strong>${escapeHtml(shortRun(failure.run))}</strong><br>${escapeHtml(failure.text)}</li>`,
          )
          .join("")}
      </ul>
    `;
  }

  function matrixCell(text) {
    const cell = document.createElement("span");
    cell.className = "matrix-cell";
    cell.textContent = text;
    return cell;
  }

  function matrixCellText(result) {
    const metrics = result.metrics || {};
    const prefix = result.passed ? "PASS" : "FAIL";
    return `${prefix} · ${formatNumber(metrics.total_tokens || 0, 0)} tok`;
  }

  function statusPill(pass, text) {
    return `<span class="status ${pass ? "pass" : "fail"}">${escapeHtml(text)}</span>`;
  }

  function traceEventCount(run) {
    return (run.results || []).reduce((sum, result) => sum + (result.events || []).length, 0);
  }

  function latestCaseMetricItems(run, select) {
    return (run.results || [])
      .map((result) => ({
        label: result.case_id,
        value: select(result),
      }))
      .sort((a, b) => number(b.value) - number(a.value))
      .slice(0, 12);
  }

  function traceSegments(events) {
    const groups = new Map([
      ["Tool", { label: "Tool", value: 0, color: "var(--accent)" }],
      ["Tokens", { label: "Tokens", value: 0, color: "var(--warn)" }],
      ["Harness", { label: "Harness", value: 0, color: "var(--danger)" }],
      ["Lifecycle", { label: "Lifecycle", value: 0, color: "var(--muted)" }],
    ]);
    for (const event of events) {
      const kind = eventKindName(event);
      if (kind.includes("Tool") || kind.includes("Approval")) {
        groups.get("Tool").value += 1;
      } else if (kind.includes("Tokens")) {
        groups.get("Tokens").value += 1;
      } else if (kind.includes("Harness") || kind.includes("Error")) {
        groups.get("Harness").value += 1;
      } else {
        groups.get("Lifecycle").value += 1;
      }
    }
    return Array.from(groups.values()).filter((segment) => segment.value > 0);
  }

  function eventKindName(event) {
    const kind = event?.kind;
    if (typeof kind === "string") {
      return kind;
    }
    if (!kind || typeof kind !== "object") {
      return "Unknown";
    }
    return Object.keys(kind)[0] || "Unknown";
  }

  function eventPayload(event) {
    const kind = event?.kind;
    if (!kind || typeof kind !== "object") {
      return null;
    }
    const key = Object.keys(kind)[0];
    return key ? kind[key] : null;
  }

  function eventSummary(event) {
    const kind = eventKindName(event);
    const payload = eventPayload(event);
    if (payload == null) {
      return "";
    }
    if (kind === "ToolRequested" || kind === "ToolStarted") {
      return `${payload.tool_name || payload.toolName || "tool"} ${payload.id || ""}`.trim();
    }
    if (kind === "ToolCompleted") {
      return `${payload.tool_name || payload.toolName || "tool"} ok=${String(payload.ok)}`;
    }
    if (kind === "TokensUpdated") {
      return `${payload.input_tokens || 0} in / ${payload.output_tokens || 0} out`;
    }
    if (kind === "HarnessStopped") {
      return payload.reason || payload.message || "";
    }
    if (kind === "Error") {
      return payload.message || "";
    }
    return JSON.stringify(payload).slice(0, 180);
  }

  function traceClass(kind) {
    if (kind.includes("Completed")) {
      return "done";
    }
    if (kind.includes("Error") || kind.includes("Stopped")) {
      return "bad";
    }
    if (kind.includes("Tool")) {
      return "tool";
    }
    return "";
  }

  function verifierOk(verifier) {
    return verifier && (verifier.status === "ok" || verifier.status === "pass" || verifier.status === "success");
  }

  function shortRun(run) {
    return String(run?.run_id || "run").replace(/^bench-/, "");
  }

  function benchmarkLabel(run) {
    const provider = run?.provider || "provider";
    const model = run?.model || run?.suite_name || "model";
    return `${provider}/${model}`;
  }

  function compactModelLabel(run) {
    const label = benchmarkLabel(run);
    return label.length > 20 ? `${label.slice(0, 17)}...` : label;
  }

  function compactLabel(value) {
    const text = String(value || "");
    if (text.length <= 14) {
      return text;
    }
    return `${text.slice(0, 11)}...`;
  }

  function passDelta(previous, latest) {
    const delta = number(latest.metrics.verified_success_rate) - number(previous.metrics.verified_success_rate);
    return `${delta >= 0 ? "+" : ""}${(delta * 100).toFixed(1)} pts vs previous`;
  }

  function deltaText(metric) {
    if (state.runs.length < 2) {
      return "No previous run";
    }
    const previous = nullable(state.runs[state.runs.length - 2].metrics[metric]);
    const current = nullable(state.runs[state.runs.length - 1].metrics[metric]);
    if (!Number.isFinite(previous) || !Number.isFinite(current)) {
      return "Delta unavailable";
    }
    const delta = current - previous;
    return `${delta >= 0 ? "+" : ""}${formatNumber(delta, 1)} vs previous`;
  }

  function copyText(text) {
    if (!navigator.clipboard) {
      return;
    }
    navigator.clipboard.writeText(text).catch(() => {});
  }

  function formatDate(value) {
    const millis = number(value);
    if (!millis) {
      return "n/a";
    }
    return new Date(millis).toLocaleString();
  }

  function duration(ms) {
    const value = number(ms);
    if (!value) {
      return "0ms";
    }
    if (value < 1000) {
      return `${value}ms`;
    }
    if (value < 60_000) {
      return `${(value / 1000).toFixed(1)}s`;
    }
    return `${(value / 60_000).toFixed(1)}m`;
  }

  function percent(value) {
    return `${(number(value) * 100).toFixed(1)}%`;
  }

  function optionalNumber(value, digits) {
    const parsed = nullable(value);
    return Number.isFinite(parsed) ? formatNumber(parsed, digits) : "n/a";
  }

  function formatNumber(value, digits = 0) {
    return number(value).toLocaleString(undefined, {
      maximumFractionDigits: digits,
      minimumFractionDigits: digits,
    });
  }

  function number(value) {
    return Number.isFinite(Number(value)) ? Number(value) : 0;
  }

  function nullable(value) {
    return value === null || value === undefined ? NaN : Number(value);
  }

  function clamp(value, min, max) {
    return Math.max(min, Math.min(max, value));
  }

  function escapeHtml(value) {
    return String(value ?? "")
      .replace(/&/g, "&amp;")
      .replace(/</g, "&lt;")
      .replace(/>/g, "&gt;")
      .replace(/"/g, "&quot;")
      .replace(/'/g, "&#039;");
  }
})();
