# Project Status

This file tracks the high-level delivery state of VectorCode's benchmark
phases. Each phase is a discrete, reviewable deliverable tracked under
`openspec/changes/` and the SDD roadmap.

| Phase | Description | Status |
|-------|-------------|--------|
| 4.1 | Public reproducible benchmark (mock-mini regression gate) | Done — see `docs/benchmarks.md` |
| 4.2 | Security audit (path boundary, symlink leak, no-unwrap) | Done — see `docs/SECURITY.md` |
| 4.4 | Real-corpus verification (thiserror / defu / itsdangerous / vscode with a pinned model) | Planned |

The pre-4.x phases (1–3) shipped the benchmark harness, the parameterized
store benchmark, the latency tracking, and the graph benchmark. The
historical ROI / agent numbers in the Fases tables of `README.md` are
unchanged from those phases — they describe real LLM runs that are not
reproducible on a clean clone today.
