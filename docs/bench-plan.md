# Plan de Implementación de Benchmarks para VectorCode

## Plan General (Roadmap)

El objetivo de este roadmap es validar el ROI, la precisión y el impacto de VectorCode de manera progresiva, yendo desde pruebas matemáticas aisladas hasta pruebas de agente completo. Cada hito (Fase) será tratado como un proyecto independiente ejecutado bajo su propio flujo SDD (`/sdd-new`).

1. **Hito 1: Benchmarks Aislados de Precisión (IR)**. Validar el motor en crudo.
2. **Hito 2: Benchmarks de Agente End-to-End**. Validar el ahorro de tokens (ROI) simulando la Tarea del Imitador.
3. **Hito 3: Benchmark de "Context Bloat"**. Validar el rendimiento en resúmenes de contexto global usando un IA Judge.

---

## Plan Detallado: Fase 1 (Precisión IR)

**Objetivo:** Probar si la matemática de los vectores y el motor de búsqueda devuelven el código correcto sin alucinaciones y con baja latencia.

**Flujo SDD a Utilizar:** `/sdd-new "Benchmark de Precisión VectorCode (Fase 1)"`

**Entregables y Tareas:**
1. **Generación del Dataset (`benchmarks/eval-dataset.json`)**
   - Crearemos un JSON con 50 pares de "Pregunta natural" vs "Ruta esperada".
   - *Ejecución:* Haremos un barrido manual o con un script asistido por IA para buscar 50 conceptos clave en el repo (ej. "lógica de retries de pagos" -> `src/payments/retry.rs`) y armaremos el archivo de evaluación.
2. **Script Evaluador (`benchmarks/phase1_precision.py` o `.sh`)**
   - Escribiremos un script que inicialice el indexado (`vectorcode index`) midiendo el tiempo de "Cold Indexing" (Métrica 1).
   - Iterará sobre el `eval-dataset.json`. Para cada query, llamará al CLI/MCP de VectorCode y medirá la Latencia de búsqueda (Métrica 2 - meta < 100ms).
   - Analizará los resultados top para calcular P@1, P@3 y P@5 evaluando si el `expected_file` se encuentra en esa posición (Métrica 3).
3. **Medición de RAM**
   - Integraremos en el script una llamada para medir el consumo del proceso mientras responde a la ráfaga de 50 queries.
4. **Reporte**
   - El script escupirá un log final. Se usará un `sdd-archive` para consolidar esos números en la fila 1 de la tabla del `README.md`.

---

## Plan Detallado: Fase 2 (Ahorro de Tokens y Agente E2E)

**✅ COMPLETADO — Commit `a121c5e` (2026-06-12)**

**Objetivo:** Demostrar que un agente usando VectorCode consume drásticamente menos tokens (Input Tokens) y comete menos errores al imitar convenciones, comparado con un agente que usa `grep` y `read_file`.

**Flujo SDD a Utilizar:** `/sdd-new "Benchmark End-to-End Tarea del Imitador (Fase 2)"`

**Entregables y Tareas:**
1. **Script Simulador de Agentes (`benchmarks/run_openai.py` / `benchmarks/run_anthropic.py`)**
   - ✅ Implementado: Harness de dos brazos con secuencias programadas (6 steps Arm A, 5 steps Arm B).
   - *Brazo A (Baseline):* `grep`, `read_file`, `generate`.
   - *Brazo B (VectorCode):* `vec_search`, `read_file`, `generate`.
   - Token counting con `tiktoken` (cl100k_base) + fallback chars/4.
   - Quality evaluator con 7 reglas de convención desde `install.rs`.
   - 10 tests unitarios inline.
2. **Parser de Sesiones (`benchmarks/_lib/parse_session.py`)**
   - ✅ Implementado: 214 líneas, Node.js ES module.
   - Suma tokens totales y de exploración, cuenta pasos de exploración antes de generación.
   - 5 tests inline. Output match entre Python y Node.js verificado.
3. **Evaluación de Calidad**
   - ✅ Implementado: 7 reglas de convención (anyhow::Result, clap::Args, derive, struct name, execute sig, test module, no unwrap).
   - Resultado: Arm B 100% (7/7), Arm A 86% (6/7) — Arm B mejor por incluir `#[cfg(test)] mod tests`.
4. **Reporte**
   - ✅ Reporte en `benchmarks/results/phase2_report.json`.
   - ✅ README.md actualizado con tabla de resultados.
   - ✅ Dry-run validado: JSONL + reporte con schema completo.
   - ✅ Cross-check: Node.js parser vs Python output — 100% match (10556/10556 tokens, etc.).

**Resultados Clave:**
- Tool calls: 6 (Arm A) → 5 (Arm B), −16.7%
- Pasos de exploración: 5 → 4, −20.0%
- Calidad de convenciones: 86% → 100%, +14%
- Tokens totales (dry-run): ~10.5k ambos (read_file domina — live mode necesario para ver savings reales)

---

## Plan Detallado: Fase 3 (Saturación de Contexto)

> ⬜ **Pendiente** — Pendiente de la Fase 2. Se recomienda revisar lessons learned de Fase 2 antes de comenzar.

**Objetivo:** Probar el impacto en tareas de entendimiento global (Context Bloat), demostrando que VectorCode evita que el agente colapse su contexto o sufra el problema de "Lost in the Middle".

**Flujo SDD a Utilizar:** `/sdd-new "Benchmark Context Bloat y AI Judge (Fase 3)"`

**Entregables y Tareas:**
1. **Prompt de Referencia y Baseline**
   - Definiremos una tarea compleja: "Resume todos los lugares en el frontend y backend donde se hace referencia a la pasarela de pagos de Stripe".
   - Armaremos manualmente (o validaremos) el "Golden Summary" con todas las referencias reales.
2. **Ejecución y Extracción**
   - Ejecutaremos el prompt en dos escenarios. Escenario sin VectorCode (que probablemente vuelque archivos enteros) y con VectorCode.
   - Extraeremos la respuesta final del LLM de cada escenario.
3. **AI Judge Evaluator (`benchmarks/run_openai.py --phase p3`)**
   - Escribiremos un script que envíe ambas respuestas junto con el "Golden Summary" a un modelo evaluador (JD-Judge-A / `jd-judge-a`).
   - El juez IA estará instruido para calificar la Precisión y Completitud (0-100) y detectar alucinaciones.
4. **Reporte**
   - Consolidar la puntuación final del AI Judge en la fila 3 de la tabla del `README.md`.

## Open Questions (Resueltas)

> ~~¿Hay alguna preferencia sobre el lenguaje para los scripts de los benchmarks?~~ ✅ **Resuelto:** Todo implementado en Python (harness, parsing y generador de reportes) usando el paquete compartido `_lib/`.
> ~~¿Las pruebas de la Fase 2 las ejecutaremos contra un modelo LLM real?~~ ✅ **Resuelto:** Secuencias programadas (scripted sequences) sin LLM real para mantener determinismo, bajo costo y repetibilidad.
