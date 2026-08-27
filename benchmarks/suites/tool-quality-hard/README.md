# Tool-quality HARD suite

Multi-bug / multi-module repairs for tool-use quality comparison.

Cases: interval merge, cache epochs, state machine, min-heap, mini-DB,
rate limiter, path VFS, expression parser.

With DeepSeek V4 Flash free + tools + `cargo test`, **pass rate often
saturates**; compare **tool counts and wall time** (see
`benchmarks/runs/agent-compare/RESULTS.md`).

```bash
just bench-tool-quality-hard
# navi only:
python3 benchmarks/scripts/run_agent_comparison.py \
  --suite benchmarks/suites/tool-quality-hard \
  --agents navi \
  --navi-bin ./target/release/navi \
  --out benchmarks/runs/agent-compare/hard-navi.json
```
