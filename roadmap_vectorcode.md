# Roadmap — VectorCode (CLI Rust + MCP, Clean Architecture)

> Objetivo: llevar el proyecto al estado del arte real (junio 2026) en búsqueda de código para agentes IA, combinando hybrid retrieval, knowledge graph de código, escala multi-repo y un enfoque local-first verificable.

---

## Pilares del proyecto

Estos principios gobiernan cada decisión técnica del roadmap. Si una feature nueva no encaja en alguno, probablemente no debería entrar todavía.

1. **Local-first, sin excepciones silenciosas** — Todo funciona 100% offline por defecto (indexado, embeddings vía Ollama, búsqueda, MCP server). Las APIs externas son *opt-in* explícito, nunca un fallback automático. Cero telemetría sin consentimiento.

2. **Retrieval que se mide, no que se siente** — Cada cambio al pipeline de búsqueda se valida contra un benchmark reproducible, no contra "probé tres queries y se veía bien".

3. **Estructura del código como ciudadano de primera clase** — El código no es texto: es AST + grafo de relaciones (llamadas, imports, herencia). El vector search responde "qué se parece a esto"; el grafo responde "qué depende de esto". Ambos se complementan, no se sustituyen.

4. **Escala real, no escala de demo** — Indexado incremental, multi-repo, footprint de memoria/disco controlado. Pensado para CI y monorepos grandes, no solo el repo de ejemplo.

5. **Arquitectura como contrato, no como decoración** — Los puertos (embeddings, vector store, grafo, parsing) están desacoplados de las implementaciones desde el día 1, aunque al principio solo exista una implementación de cada uno.

6. **MCP como interfaz, no como producto** — El motor de búsqueda/grafo debe ser útil también vía CLI directo, sin depender de un cliente MCP.

7. **Honestidad sobre el estado** — Cada release documenta claramente qué tan completo está cada pilar. Nada de "production-ready" prematuro.

---

## Estado actual (línea base verificada)

Snapshot al cierre de fase 4.3 (commit `9df149a`). El detalle por pilar
—verdict %, evidencia con `file:line` y límites conocidos— vive en
[`docs/STATUS.md`](docs/STATUS.md) y los siete deep dives bajo
`docs/pilar-status/`. El snapshot pre-Fase-1 (que tenía 4 entradas
marcadas como pendientes y que el código ya había resuelto) se preserva
al pie del archivo como `## Historical snapshots`.

- ✅ Arquitectura hexagonal, tests compilando y en verde
- ✅ MCP con 8 tools (3 graph-aware), `on_initialized` proactivo, `parent_context` en resultados
- ✅ AST chunking vía Tree-sitter, 14 lenguajes; 3 de ellos con grafo (Rust / TS-JS / Python)
- ✅ Seguridad: path canonization, boundary checks, límites de lectura, BTreeMap de workspaces
- ✅ Embedder con traits — 6 proveedores (Onnx, Ollama, OpenAI, Gemini, OpenRouter, Mock)
- ✅ Store: trait `Store` con 2 impls (`SqliteStore` producción, `LanceStore` SHIM tras `--features lancedb-store`); decisión documentada en ADR-0001
- ✅ Hybrid search: dense + sparse (FTS5) + RRF + reranker ONNX (BGE-Reranker-v2-m3)
- ✅ Knowledge graph de relaciones: nodos + aristas `Call` / `Import` extraídos inline durante el indexado
- ✅ Multi-repo serve: `BTreeMap<PathBuf, AppInnerState>` con `repo_name` por resultado
- ✅ Benchmark formal: harness en `src/bench/`, baseline en `BASELINE.md` + `benchmarks/baseline/`, regression gate en `scripts/verify-baseline.sh`

---

## Fase 1 — Hybrid Search + RRF + Reranker corregido + Benchmark base

**Objetivo:** llevar el motor de búsqueda de "dense only" a estado del arte real, y poder probarlo con números, no con intuición.

| # | Tarea | Detalle |
|---|---|---|
| 1.1 | Benchmark harness propio | Repo de prueba fijo + set de queries con respuestas esperadas (estilo RepoEval reducido). Métricas: Recall@5, Recall@10, nDCG. Este harness se corre en cada fase siguiente — es el termómetro de "estado del arte". |
| 1.2 | Baseline medido | Correr el benchmark contra el dense search actual. Este número es el punto de comparación para todo lo que sigue. |
| 1.3 | Sparse search (FTS5) | Nueva tabla FTS5 en SQLite, indexada en paralelo al vector store durante el indexing. Puerto `SparseSearcher` separado del `DenseSearcher` (contrato explícito, pilar 5). |
| 1.4 | Fusión RRF | `engine/fusion.rs` nuevo. Implementar RRF (K configurable, default 60) combinando resultados de dense + sparse. |
| 1.5 | Reranker dedicado vía ONNX | Modelo cross-encoder/listwise especializado, no un LLM generalista — elimina por diseño el bug de concurrencia de Proyecto B (no hay loop de N llamadas a un LLM que controlar; es una sola inferencia local). Default: Qwen3-Reranker-0.6B (Apache 2.0, evaluado en retrieval de código vía MTEB-Code). Alternativa configurable: BGE-Reranker-v2-m3 (Apache 2.0, 568M, más tiempo en producción). Puerto Reranker como trait (pilar 5) para poder intercambiar el modelo sin tocar el resto del pipeline. Corre vía ort (ONNX Runtime para Rust), CPU-only por defecto — coherente con el pilar local-first, sin pedir GPU al usuario. Timeout explícito + fallback a orden RRF si el reranker falla. |
| 1.6 | Re-medir con benchmark | Confirmar mejora real de Recall/nDCG vs baseline 1.2. Si no mejora, el reranker no se queda. |
| 1.7 | Nueva MCP tool: `vec_search` con modo configurable | Exponer `mode: dense \| hybrid \| hybrid+rerank` para que el agente o usuario elija el trade-off latencia/calidad explícitamente. |

**Criterio de salida:** benchmark mostrando mejora medible sobre dense-only, reranker sin riesgo de latencia descontrolada, tests verdes.

---

## Fase 2 — Knowledge Graph de código

**Objetivo:** complementar "qué se parece a esto" (vectores) con "qué depende de esto" (grafo).

| # | Tarea | Detalle |
|---|---|---|
| 2.1 | Extracción de relaciones vía Tree-sitter | Durante el chunking ya se tiene el AST — extraer también: llamadas a función, imports, herencia/implementación, referencias a símbolos. Nuevo módulo `engine/graph_extractor.rs`. |
| 2.2 | Modelo de grafo | Nodos = símbolos (función, clase, módulo). Edges = `calls`, `imports`, `extends`, `references`. Decidir almacenamiento (tabla relacional en SQLite vs estructura en memoria con persistencia periódica) con datos reales del repo de benchmark. |
| 2.3 | Puerto `GraphStore` | Trait desacoplado, para no atar la implementación del grafo a una sola tecnología (pilar 5). |
| 2.4 | Nuevas MCP tools | `vec_find_callers`, `vec_find_dependents`, `vec_trace_imports` — tools puntuales, consistentes con la fortaleza actual de tools granulares tipo `vec_outline`/`vec_read_lines`. |
| 2.5 | Fusión retrieval + grafo | Heurística de ruteo: preguntas estructurales ("qué llama a X") priorizan grafo; preguntas conceptuales ("dónde se maneja auth") priorizan hybrid search. |
| 2.6 | Benchmark extendido | Añadir al harness de Fase 1 preguntas estructurales para medir el grafo con el mismo rigor que el retrieval semántico. |

**Criterio de salida:** preguntas estructurales respondidas con precisión vía grafo, sin degradar el rendimiento del hybrid search existente.

---

## Fase 3 — Multi-repo + indexado incremental + decisión de store

**Objetivo:** escalar lo que ya funciona bien en un repo a N repos, sin reescribir el motor.

| # | Tarea | Detalle |
|---|---|---|
| 3.1 | Evaluación de store a escala | Con datos reales de Fase 1-2 (tamaño de embeddings + FTS + grafo), probar sqlite-vec vs LanceDB vs Qdrant embebido en un repo grande real. Decisión basada en benchmarks de tiempo de indexado + memoria. |
| 3.2 | Migración de store (si aplica) | Si 3.1 indica migrar, hacerlo detrás del puerto `Store` ya existente — el resto del motor no debería notar el cambio. |
| 3.3 | Indexado incremental real | Hashing de archivos (BLAKE3 — `src/types.rs:127` y `:134`) + timestamps para reindexar solo lo que cambió. |
| 3.4 | Multi-repo serve mode | El MCP server indexa y sirve búsquedas sobre varios repos a la vez, con resultados que indican de qué repo vienen. Ranking cruzado vía RRF. |
| 3.5 | Footprint y CI-readiness | Medir tiempo de indexado y consumo de memoria en un monorepo grande real. Validar que corre en un runner de CI estándar. |
| 3.6 | Benchmark a escala | Extender el harness para medir Recall/latencia con múltiples repos cargados simultáneamente. |

**Criterio de salida:** indexado incremental funcionando, decisión de store tomada con datos, multi-repo operativo y medido en condiciones reales de CI/monorepo.

---

## Fase 4 — Hardening, benchmarks formales y release

**Objetivo:** pasar de "funciona y se mide internamente" a "puedo defenderlo públicamente como estado del arte".

| # | Tarea | Detalle |
|---|---|---|
| 4.1 | Benchmark público reproducible | Publicar el harness como parte del repo, con instrucciones para que cualquiera lo corra y verifique los números. |
| 4.2 | Auditoría de seguridad | Revisar de nuevo boundary checks y límites de lectura; validar si el grafo o multi-repo abren alguna superficie nueva de escape de directorio. |
| 4.3 | Documentación honesta por pilar | Por cada pilar, documentar explícitamente qué tan completo está, con `file:line` por afirmación y `Known limits` no vacío. Ver [`docs/STATUS.md`](docs/STATUS.md) + `docs/pilar-status/P{1..7}-*.md` (P1-P7). |
| 4.4 | Comparación pública vs competidores | Correr el benchmark contra `flupkede/codesearch`, `mcp-vector-search`, etc., para tener una comparación objetiva. |
| 4.5 | Release v1.0 | Changelog, guía de instalación, ejemplos de configuración Ollama+API, guía de contribución. |

**Criterio de salida:** alguien externo puede clonar el repo, correr el benchmark, y verificar las afirmaciones de calidad por sí mismo.

---

## Notas de ejecución

- **Cada fase termina con el benchmark corriendo y mejorando o manteniéndose.** Si una fase empeora el benchmark de una anterior, eso es una regresión que bloquea el avance, no un detalle a anotar para después.
- **Los puertos/traits se definen incluso para lo que no se va a implementar todavía** (ej. `GraphStore` trait existe desde que se toca el grafo, aunque la única implementación sea SQLite al principio) — esto evita reescribir el core en la Fase 3.
- El orden grafo → multi-repo (en vez de multi-repo → grafo) tiene una ventaja adicional: al llegar a la Fase 3 y evaluar LanceDB/Qdrant, ya se conoce el shape completo de datos a persistir (vectores + FTS + grafo), evitando una segunda migración de store.

---

## Contexto de referencia — Análisis comparativo previo

Este roadmap parte de un análisis comparativo entre dos arquitecturas (Proyecto A = base actual, Proyecto B = referencia de hybrid search) que motivó la Fase 1:

- **Proyecto A (base actual):** arquitectura hexagonal sólida, MCP excepcional (5 tools, `on_initialized` proactivo, `parent_context`), AST chunking en 12+ lenguajes, seguridad robusta. Debilidad principal: solo dense search.
- **Proyecto B (referencia):** pipeline de hybrid search avanzado (FTS5 + RRF + reranking), pero con un bug crítico de concurrencia en el reranker (instanciación de `LlmClient` dentro del loop sin pool/Arc, `tokio::spawn` sin control) que generaba latencias severas. La Fase 1 porta la idea de B corrigiendo explícitamente ese error de diseño.

---

## Historical snapshots

> Snapshots preservados por auditoría. La sección `## Estado actual` de este
> archivo se reescribe en cada fase para reflejar la verdad; los snapshots
> históricos quedan aquí para que un lector pueda reconstruir el "antes" sin
> reconstruirlo a mano.

### Estado al inicio (Fase 1) — historical snapshot

Snapshot del bloque `## Estado actual (línea base verificada)` previo al
commit C2 de la fase 4.3. Contenía 4 `❌` que el código ya había
resuelto: dense-only (Fase 1), knowledge graph (Fase 2), multi-repo
(Fase 3), benchmark formal (Fase 1.2). El snapshot original se
preserva verbatim abajo; el bloque actual está al inicio del archivo.

```text
- ✅ Arquitectura hexagonal, tests compilando y en verde
- ✅ MCP con 5 tools, `on_initialized` proactivo, `parent_context` en resultados
- ✅ AST chunking vía Tree-sitter, 12+ lenguajes
- ✅ Seguridad: path canonization, boundary checks, límites de lectura
- ✅ Embedder con traits (Ollama, OpenAI, Gemini, ONNX)
- ✅ Store: SQLite + sqlite-vec
- ❌ Solo dense search (sin BM25/FTS, sin RRF, sin reranking)
- ❌ Sin knowledge graph de relaciones
- ❌ Sin soporte multi-repo
- ❌ Sin benchmark formal de calidad de retrieval
```
