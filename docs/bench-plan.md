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

**Objetivo:** Demostrar que un agente usando VectorCode consume drásticamente menos tokens (Input Tokens) y comete menos errores al imitar convenciones, comparado con un agente que usa `grep` y `read_file`.

**Flujo SDD a Utilizar:** `/sdd-new "Benchmark End-to-End Tarea del Imitador (Fase 2)"`

**Entregables y Tareas:**
1. **Script Simulador de Agentes (`benchmarks/phase2_token_savings.py`)**
   - No podemos depender de agentes manuales. Crearemos un harness que le pase un prompt ("Agrega un endpoint DELETE /users/:id siguiendo las mismas convenciones que products") a dos agentes programáticos (Brazo A y Brazo B).
   - *Brazo A (Baseline):* Se le provee `bash`, `read_file`, `grep`, `find`.
   - *Brazo B (VectorCode):* Se le provee `vec_search` y `read_file`.
2. **Parser de Sesiones (`benchmarks/agent-eval/parse-session.mjs`)**
   - Construiremos un parser en Node.js que analice los logs (JSONL/text) de ambas sesiones.
   - Sumará el "Costo Total del Contexto" (Prompt tokens enviados).
   - Contará la "Eficiencia de Exploración" (cantidad de tool calls de búsqueda hechos hasta empezar a escribir).
3. **Evaluación de Calidad**
   - El simulador extraerá el código generado y validará si cumple con las convenciones de `products` (ej. si la clase de error retornada es correcta o genérica).
4. **Reporte**
   - Consolidar los resultados (tokens ahorrados, reducción de tool calls) en la fila 2 de la tabla del `README.md`.

---

## Plan Detallado: Fase 3 (Saturación de Contexto)

**Objetivo:** Probar el impacto en tareas de entendimiento global (Context Bloat), demostrando que VectorCode evita que el agente colapse su contexto o sufra el problema de "Lost in the Middle".

**Flujo SDD a Utilizar:** `/sdd-new "Benchmark Context Bloat y AI Judge (Fase 3)"`

**Entregables y Tareas:**
1. **Prompt de Referencia y Baseline**
   - Definiremos una tarea compleja: "Resume todos los lugares en el frontend y backend donde se hace referencia a la pasarela de pagos de Stripe".
   - Armaremos manualmente (o validaremos) el "Golden Summary" con todas las referencias reales.
2. **Ejecución y Extracción**
   - Ejecutaremos el prompt en dos escenarios. Escenario sin VectorCode (que probablemente vuelque archivos enteros) y con VectorCode.
   - Extraeremos la respuesta final del LLM de cada escenario.
3. **AI Judge Evaluator (`benchmarks/phase3_context_bloat.py`)**
   - Escribiremos un script que envíe ambas respuestas junto con el "Golden Summary" a un modelo evaluador (JD-Judge-A / `jd-judge-a`).
   - El juez IA estará instruido para calificar la Precisión y Completitud (0-100) y detectar alucinaciones.
4. **Reporte**
   - Consolidar la puntuación final del AI Judge en la fila 3 de la tabla del `README.md`.

## Open Questions

> [!WARNING]
> - ¿Hay alguna preferencia sobre el lenguaje para los scripts de los benchmarks? Propongo **Python** para las Fases 1, 2 y 3 (para interactuar fácil con APIs y subprocess) y **Node.js** para el `parse-session.mjs` como sugeriste.
> - ¿Las pruebas de la Fase 2 las ejecutaremos contra un modelo LLM real (ej. invocando la API de Gemini u OpenAI) y pagaremos el costo, o las simularemos de alguna otra forma?
