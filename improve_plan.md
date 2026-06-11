# VectorCode — Production Readiness Improve Plan

> **Objetivo**: Llevar VectorCode v0.1.0 a un estado production-ready cubriendo todos los pendientes de la sección 18 del as-built document.

---

## My Opinion — Architectural Review

Antes de entrar al plan, mi lectura honesta del proyecto:

> [!TIP]
> **Lo bueno — y hay MUCHO bueno aquí**

- **375 tests, 0 clippy warnings, strict TDD.** Esto no es un MVP tirado con duct tape. Cada módulo tiene tests unitarios sólidos, los integration tests del MCP son reales (spawn del binario, JSON-RPC), y el coverage estimado >80% es creíble por la forma en que se escribieron.
- **Arquitectura limpia.** La separación `cli/`, `mcp/`, `engine/`, `store/`, `embedder/`, `watcher/`, `config/` es exactamente lo que esperarías de un proyecto que va a escalar. No hay globals, no hay estado compartido fuera de `AppState`, y el `Embedder` trait es un diseño de libro.
- **Error handling.** `VectorCodeError` con 10 variantes tipadas, `From` impls para rusqlite y io, mensajes actionable para el usuario. Esto es lo que quiero ver en producción.
- **Pragmatismo en las decisiones.** El fallback `vectors_data` con JSON mientras sqlite-vec no está integrado fue la decisión correcta — entregar funcionalidad sin bloquear en una extensión C.

> [!WARNING]
> **Lo que necesita trabajo**

- **El elefante en la sala: sin ONNX model bundled, el provider default no funciona.** El binario se construye, pero `vectorcode init` con el provider default termina usando `MockEmbedder`. Esto es el blocker #1.
- **Brute-force vector search.** Funciona para demos y proyectos chicos (<5K chunks), pero un monorepo de 50K+ chunks va a ser inaceptablemente lento. sqlite-vec no es optional — es NECESARIO para el caso de uso target.
- **El flaky test es un code smell.** `temp_env_var` con closures no es thread-safe. En CI con `--test-threads=N` esto va a explotar. Es un fix rápido pero bloqueante para CI.
- **El `upgrade` stub.** Sin self-update, la distribución es manual. No es blocker para v0.1, pero sí para adoption.

> [!NOTE]
> **Veredicto**: El proyecto está bien construido. La deuda técnica es CONOCIDA y DOCUMENTADA (lo cual es mejor que la mayoría de los proyectos que veo). El camino a producción es claro y acotado.

---

## Decisions (Confirmed)

| Decision | Choice | Rationale |
|----------|--------|-----------|
| **ONNX model** | Download on-demand during `vectorcode init` | Binary liviano (~17MB), el usuario elige provider interactivamente |
| **sqlite-vec** | Compilación estática en `build.rs` | Zero runtime deps, self-contained binary |
| **GitHub org** | `alejandro-technology/vectorcode` | Releases, CI, self-update apuntan aquí |
| **Homebrew** | Tap separado: `alejandro-technology/homebrew-vectorcode` | Estándar para herramientas propias |
| **Self-update** | GitHub Releases binaries pre-compilados | Descarga + SHA256 verify + replace |

---

## Proposed Changes

Las fases están ordenadas por dependencia — cada fase se puede implementar y testear independientemente.

---

### Phase 1: Known Issues & Foundation Fixes

Arreglar problemas conocidos antes de agregar funcionalidad nueva.

#### [MODIFY] [mod.rs](file:///Users/alejandro/Documents/PROJECTS/MCP/vector-code/src/config/mod.rs)
- Reemplazar `temp_env_var` helper con `serial_test` o `temp-env` crate para eliminar el race condition del test flaky (§19.1)
- Los tests que modifican env vars se ejecutarán en serie

#### [NEW] LICENSE
- Crear archivo `LICENSE` con texto MIT (§18.2.3 — trivial pero bloqueante para open source)

---

### Phase 2: ONNX Model On-Demand Download (CRITICAL §18.1.1)

El modelo NO se embebe en el binario. Se descarga la primera vez que el usuario ejecuta `vectorcode init` con provider `onnx`.

#### [NEW] src/embedder/model_manager.rs
- `ModelManager` struct con lógica de descarga y cache:
  - `model_dir()` → `~/.vectorcode/models/minilm-l6-v2-q8/`
  - `is_downloaded() -> bool` — verifica si `model.onnx` y `tokenizer.json` existen
  - `download_model() -> Result<()>` — descarga desde HuggingFace:
    - `model.onnx`: `https://huggingface.co/sentence-transformers/all-MiniLM-L6-v2/resolve/main/onnx/model_quantized.onnx`
    - `tokenizer.json`: `https://huggingface.co/sentence-transformers/all-MiniLM-L6-v2/resolve/main/tokenizer.json`
  - Progress bar durante descarga (indicatif)
  - Verificación de integridad post-descarga (file size sanity check)
  - `load_model() -> Result<(Vec<u8>, Vec<u8>)>` — lee archivos del cache

#### [MODIFY] [onnx.rs](file:///Users/alejandro/Documents/PROJECTS/MCP/vector-code/src/embedder/onnx.rs)
- Agregar `OnnxEmbedder::from_cache()` que usa `ModelManager` para cargar desde `~/.vectorcode/models/`
- Mantener `OnnxEmbedder::new(model_bytes, tokenizer_bytes)` para custom models

#### [MODIFY] [init.rs](file:///Users/alejandro/Documents/PROJECTS/MCP/vector-code/src/cli/init.rs)
- **Interactive provider selection** cuando no se pasa `--provider`:
  ```
  Select embedding provider:
  1. onnx    — Local, offline, no API key needed (~23MB download)
  2. gemini  — Google API, requires GEMINI_API_KEY
  3. ollama  — Local Ollama server, requires `ollama serve`
  4. openai  — OpenAI API, requires OPENAI_API_KEY
  > 
  ```
- Si elige `onnx` y el modelo no está descargado → llamar `ModelManager::download_model()`
- Si elige API provider → pedir API key interactivamente y guardar en config
- Si `--provider` se pasa como argumento, skip el prompt interactivo

#### [MODIFY] [mod.rs](file:///Users/alejandro/Documents/PROJECTS/MCP/vector-code/src/cli/mod.rs)
- Actualizar `create_embedder_from_config` para que "onnx" use `OnnxEmbedder::from_cache()`
- Error claro si el modelo no está descargado: "Run `vectorcode init` to download the ONNX model"

---

### Phase 3: sqlite-vec Integration (CRITICAL §18.1.2)

Compilación estática — el binario incluye sqlite-vec sin deps runtime.

#### [MODIFY] [Cargo.toml](file:///Users/alejandro/Documents/PROJECTS/MCP/vector-code/Cargo.toml)
- Agregar `sqlite-vec` como build dependency para compilar la extensión C
- O usar el crate `sqlite-vec-rs` si existe en crates.io

#### [MODIFY] [build.rs](file:///Users/alejandro/Documents/PROJECTS/MCP/vector-code/build.rs)
- Descargar/incluir source de sqlite-vec (C)
- Compilar con `cc` crate como extensión estática
- Linkar con rusqlite via `auto_extension`

#### [MODIFY] [db.rs](file:///Users/alejandro/Documents/PROJECTS/MCP/vector-code/src/store/db.rs)
- Registrar sqlite-vec como auto-extension al abrir la DB:
  ```rust
  unsafe { rusqlite::ffi::sqlite3_auto_extension(Some(sqlite3_vec_init)); }
  ```
- Modificar `init_schema()` para crear `vec_chunks` virtual table con `vec0`
- Mantener `vectors_data` como fallback SOLO si la extensión falla (defensive)
- Schema migration (v1 → v2): detectar si `vectors_data` tiene datos y migrar a `vec_chunks`

#### [MODIFY] [vectors.rs](file:///Users/alejandro/Documents/PROJECTS/MCP/vector-code/src/store/vectors.rs)
- `insert_vector()`: insertar en `vec_chunks` (primary), fallback a `vectors_data` si no hay extensión
- `search_similar()`: usar query nativo sqlite-vec:
  ```sql
  SELECT c.*, v.distance
  FROM vec_chunks v
  JOIN chunks c ON c.id = v.chunk_id
  WHERE v.embedding MATCH ?query_vec AND k = ?limit
  ORDER BY v.distance ASC
  ```
- `delete_vectors_for_chunk()`: borrar de tabla activa
- Eliminar `cosine_similarity()` manual (la mantiene sqlite-vec internamente)
- Benchmark: validar 10-100x speedup vs brute-force en tests

---

### Phase 4: Self-Update (CRITICAL §18.1.3)

#### [MODIFY] [upgrade.rs](file:///Users/alejandro/Documents/PROJECTS/MCP/vector-code/src/cli/upgrade.rs)
- Implementar `check_latest_version()` via GitHub API:
  `GET https://api.github.com/repos/alejandro-technology/vectorcode/releases/latest`
- Implementar `download_and_replace()`:
  1. Detectar OS/arch actual (`std::env::consts::{OS, ARCH}`)
  2. Construir URL: `https://github.com/alejandro-technology/vectorcode/releases/download/v{ver}/vectorcode-{os}-{arch}.tar.gz`
  3. Descargar tarball
  4. Verificar checksum SHA256 contra `checksums.txt` del release
  5. Reemplazar binario actual (`self_replace` crate para atomic swap)
  6. `chmod +x` en Unix
- Flags: `--check` (solo verificar), `--version <VER>` (versión específica)

#### [MODIFY] [Cargo.toml](file:///Users/alejandro/Documents/PROJECTS/MCP/vector-code/Cargo.toml)
- Agregar `self_replace = "1"` para atomic binary replacement

---

### Phase 5: CI/CD Pipeline (HIGH §18.2.1)

#### [NEW] .github/workflows/ci.yml
- **Test job**: `cargo test`, `cargo clippy -- -D warnings`, `cargo fmt --check`
- **Build job**: matrix build para 5 targets:
  - `x86_64-apple-darwin`
  - `aarch64-apple-darwin`
  - `x86_64-unknown-linux-gnu`
  - `aarch64-unknown-linux-gnu`
  - `x86_64-pc-windows-msvc`
- **Release job** (on tag push `v*`):
  - Build release binaries para cada target
  - Strip symbols
  - Crear tarball (`.tar.gz` Unix) / zip (Windows)
  - Compute SHA256 checksums → `checksums.txt`
  - Publicar como GitHub Release en `alejandro-technology/vectorcode` con assets

---

### Phase 6: Test Coverage & Quality (HIGH §18.2.2)

#### [NEW] .github/workflows/coverage.yml
- Job con `cargo-llvm-cov` (más preciso que tarpaulin para Rust)
- Generar reporte lcov y HTML
- Upload a Codecov o similar
- Badge en README

#### [MODIFY] [Cargo.toml](file:///Users/alejandro/Documents/PROJECTS/MCP/vector-code/Cargo.toml)
- Agregar `serial_test = "3"` a dev-dependencies (para los env var tests)

---

### Phase 7: More Languages (HIGH §18.2.4)

#### [MODIFY] [Cargo.toml](file:///Users/alejandro/Documents/PROJECTS/MCP/vector-code/Cargo.toml)
- Agregar gramáticas tree-sitter: `tree-sitter-c-sharp`, `tree-sitter-c`, `tree-sitter-cpp`, `tree-sitter-ruby`, `tree-sitter-swift`, `tree-sitter-kotlin`

#### [MODIFY] [languages.rs](file:///Users/alejandro/Documents/PROJECTS/MCP/vector-code/src/engine/languages.rs)
- Extender `SupportedLanguage` enum con: `CSharp`, `C`, `Cpp`, `Ruby`, `Swift`, `Kotlin`
- Agregar lazy `OnceLock` para cada nueva gramática
- Mapear extensiones: `.cs`, `.c`, `.h`, `.cpp`, `.hpp`, `.cc`, `.rb`, `.swift`, `.kt`, `.kts`

#### [MODIFY] [chunker.rs](file:///Users/alejandro/Documents/PROJECTS/MCP/vector-code/src/engine/chunker.rs)
- Agregar chunkable node types para cada lenguaje nuevo:
  - C#: `method_declaration`, `class_declaration`, `interface_declaration`, `enum_declaration`, `namespace_declaration`
  - C: `function_definition`, `struct_specifier`
  - C++: `function_definition`, `class_specifier`, `namespace_definition`
  - Ruby: `method`, `class`, `module`, `singleton_method`
  - Swift: `function_declaration`, `class_declaration`, `protocol_declaration`, `enum_declaration`
  - Kotlin: `function_declaration`, `class_declaration`, `object_declaration`

#### [NEW] tests/fixtures/sample_cs/, sample_c/, sample_cpp/, sample_rb/, sample_swift/, sample_kt/
- Fixture files para cada lenguaje nuevo con tests de integración

---

### Phase 8: Distribution (HIGH §18.2.5 + §18.2.6)

#### [NEW] install.ps1
- Script PowerShell equivalente a `install.sh`:
  - Detectar arquitectura Windows
  - Descargar de `https://github.com/alejandro-technology/vectorcode/releases/`
  - Instalar en `$env:USERPROFILE\.vectorcode\bin\`
  - Agregar al PATH del usuario

#### [NEW] Repo: `alejandro-technology/homebrew-vectorcode`
- Homebrew formula `vectorcode.rb` con:
  - `desc`, `homepage`, `url` apuntando a `https://github.com/alejandro-technology/vectorcode/releases/`
  - `sha256` checksums por plataforma (macOS arm64 + x86_64)
  - Install from pre-compiled release tarball
- Usuarios instalan con: `brew install alejandro-technology/vectorcode/vectorcode`

---

### Phase 9: Skill & Instructions Files (MEDIUM §18.3.1 + §18.3.2)

#### [NEW] skills/semantic-search/SKILL.md
- Contenido exacto de spec §15.2

#### [MODIFY] [install.rs](file:///Users/alejandro/Documents/PROJECTS/MCP/vector-code/src/cli/install.rs)
- Agregar copia de `SKILL.md` a `.agents/skills/semantic-search/SKILL.md` (per-project)
- Agregar copia global a `~/.agents/skills/semantic-search/SKILL.md`
- Agregar escritura de `instructions.md` a `~/.gemini/antigravity/mcp/vectorcode/instructions.md`
- Contenido embebido como `const` strings (no files externos)

---

### Phase 10: UX Improvements (MEDIUM §18.3.3 + §18.3.4)

#### [MODIFY] [serve.rs](file:///Users/alejandro/Documents/PROJECTS/MCP/vector-code/src/cli/serve.rs)
- Considerar cambiar `--no-watch` por `--watch` con default true (evaluar viabilidad con clap)

#### [MODIFY] [Cargo.toml](file:///Users/alejandro/Documents/PROJECTS/MCP/vector-code/Cargo.toml)
- Agregar `indicatif = "0.17"` para progress bars

#### [MODIFY] [index.rs](file:///Users/alejandro/Documents/PROJECTS/MCP/vector-code/src/cli/index.rs)
- Reemplazar `tracing::info!` de progreso con `indicatif::ProgressBar` para el CLI
- Mantener `tracing::info!` para modo MCP (stderr)

---

### Phase 11: Low Priority Enhancements (LOW §18.4)

Estos se implementan si queda tiempo/energía. Están priorizados por impacto:

#### §18.4.5 Code-to-code search
- Agregar `kind: "code"` parameter a `vec_search` tool
- Si `kind == "code"`, embeder el snippet directamente sin enrichment

#### §18.4.2 Query enrichment avanzado
- Expandir acrónimos comunes (auth → authentication, db → database, etc.)
- Agregar sinónimos de dominio configurable

#### §18.4.3 Hybrid search (vector + FTS5)
- Agregar tabla FTS5 en `init_schema()`
- Combinar scores: `final_score = α * vector_score + (1-α) * fts_score`

#### §18.4.1 GPU acceleration
- Feature flag: `cargo build --features gpu`
- CUDA/Metal execution providers para ort

#### §18.4.6 Index compression
- Quantizar vectores a int8 post-embedding

#### §18.4.4 Cross-project search
- Nuevo tool `vec_search_multi` que acepta múltiples project paths

#### §18.4.7 Shared index server (HTTP transport)
- Alternativa a stdio para CI/multi-user

---

## Verification Plan

### Automated Tests
```bash
# All tests pass (existing + new)
cargo test

# No warnings
cargo clippy -- -D warnings

# Formatting
cargo fmt --check

# ONNX model download and embedding
cargo test -- embedder::model_manager::tests


# sqlite-vec search works
cargo test -- store::vectors::tests

# Self-update version check (mocked)
cargo test -- cli::upgrade::tests

# New language chunking
cargo test --test chunker_integration_test

# MCP integration (all tools)
cargo test --test mcp_integration_test
```

### Manual Verification
1. `cargo build --release` → binary < 50MB
2. `vectorcode init` → uses real ONNX model (not MockEmbedder)
3. `vectorcode index` → produces real embeddings, stores in sqlite-vec
4. `vectorcode search "payment retry logic"` → returns relevant results from fixtures
5. `vectorcode serve --mcp` → MCP server responds correctly
6. `vectorcode upgrade --check` → reports version info
7. CI pipeline: push tag → builds 5 platform binaries → creates GitHub Release

---

## Execution Order Summary

| Phase | Priority | Effort | Dependencies |
|-------|----------|--------|--------------|
| 1. Foundation fixes | CRITICAL | ~1h | None |
| 2. ONNX bundling | CRITICAL | ~3h | Phase 1 |
| 3. sqlite-vec | CRITICAL | ~4h | Phase 1 |
| 4. Self-update | CRITICAL | ~3h | None |
| 5. CI/CD | HIGH | ~3h | Phase 1-4 |
| 6. Test coverage | HIGH | ~1h | Phase 5 |
| 7. More languages | HIGH | ~3h | None |
| 8. Distribution | HIGH | ~2h | Phase 5 |
| 9. Skill files | MEDIUM | ~1h | None |
| 10. UX improvements | MEDIUM | ~2h | None |
| 11. Low enhancements | LOW | ~8h+ | Phases 1-3 |

**Estimación total (CRITICAL + HIGH)**: ~20h
**Estimación total (todo)**: ~31h+
