# Plan de Implementación: Agent UX (1.1, 1.2, 1.3)

> Generado: 2026-06-13 | Orden recomendado: 1.3 → 1.2 → 1.1

---

## 1.3 — Anti-Ansiedad Cognitiva (trivial)

**Archivos:** `src/cli/install.rs`, `src/mcp/handler.rs`

Agregar advertencia contra el uso secuencial de `vec_read_lines` para reconstruir archivos completos.

### Acciones

1. En `MCP_INSTRUCTIONS` (`src/cli/install.rs:73-104`), agregar bullet en la sección "Anti-patterns":
   ```
   - Don't call `vec_read_lines` sequentially (e.g., 1-100, 100-200) to
     reconstruct an entire file. Use `vec_search` to find relevant code, then
     read only the specific lines you need.
   ```

2. En `get_info()` (`src/mcp/handler.rs:294-299`), extender `instructions` con la misma advertencia.

**Esfuerzo:** 5 líneas, 5 minutos. Sin riesgos.

---

## 1.2 — `path_filter` en SQL (bajo esfuerzo)

**Archivos:** `src/store/vectors.rs`, `src/engine/searcher.rs`

El parámetro `path` ya existe en `VecSearchParams` y `SearchOptions`, pero el filtrado es post-query con `retain()`. Hay que moverlo al SQL para evitar recuperar vectores que se van a descartar.

### Acciones

#### En `src/store/vectors.rs` — función `search_similar`

1. Agregar parámetro `path_filter: Option<&str>` a la firma.

2. **Rama sqlite-vec** (Step 2, línea ~170-192): modificar el lookup de `chunk_id` vía `chunk_vec_map` para que haga JOIN con `chunks` y filtre:
   ```sql
   SELECT cvm.chunk_id FROM chunk_vec_map cvm
   JOIN chunks c ON c.id = cvm.chunk_id
   WHERE cvm.vec_rowid = ?1 AND c.file_path LIKE ?2
   ```
   Si `path_filter` es `None`, usar la query actual sin JOIN ni filtro.

3. **Rama fallback** (sin sqlite-vec, línea ~196-218): modificar la query:
   ```sql
   SELECT v.chunk_id, v.embedding FROM vectors_data v
   JOIN chunks c ON c.id = v.chunk_id
   WHERE c.file_path LIKE ?1
   ```
   Si `path_filter` es `None`, usar la query actual.

4. Usar pattern `path_filter + "%"` para el LIKE (prefijo, no substring arbitrario).

#### En `src/engine/searcher.rs` — método `search`

5. Pasar `options.path` como `path_filter` a `search_similar`.

6. Eliminar el `retain()` post-query de líneas 106-108 (el filtro ya se aplicó en SQL).

7. Ajustar `fetch_limit`: si hay `path_filter`, bajar de `limit * 5` a `limit * 2` (la query SQL ya acota el espacio).

### Tests a agregar (en `src/store/vectors.rs`)

- `search_similar_with_path_filter_vec_chunks` — solo devuelve resultados del path
- `search_similar_with_path_filter_fallback` — ídem sin sqlite-vec

**Esfuerzo:** ~30 líneas netas, 1 hora.

---

## 1.1 — Herramienta `vec_outline` (esfuerzo medio)

**Archivo nuevo:** `src/engine/outliner.rs`
**Archivos modificados:** `src/engine/mod.rs`, `src/mcp/handler.rs`

Nueva herramienta MCP `vec_outline(file_path)` que extrae las firmas de símbolos top-level (funciones, structs, enums, traits, clases) usando tree-sitter, sin los cuerpos.

### API

```rust
/// Item del esquema estructural de un archivo.
pub struct OutlineItem {
    pub kind: String,           // "function", "struct", "enum", "trait", "impl", "class"
    pub name: String,           // nombre del símbolo
    pub signature: String,      // firma completa sin cuerpo
    pub start_line: u32,
    pub visibility: Option<String>, // "pub", "pub(crate)", etc.
}

pub fn outline_file(
    source: &str,
    file_path: &str,
    language: SupportedLanguage,
) -> Result<Vec<OutlineItem>>;
```

### Acciones

#### En `src/engine/outliner.rs` (nuevo)

1. Implementar `outline_file()` recorriendo el AST con tree-sitter.

2. Para cada lenguaje, mapear qué tipos de nodo son "declarativos":

| Lenguaje | Nodos | Estrategia de corte |
|----------|-------|---------------------|
| Rust | `function_item`, `struct_item`, `enum_item`, `trait_item`, `impl_item` | Cortar en el hijo `body`/`declaration_list`, o al final de la firma si no hay cuerpo |
| TypeScript | `function_declaration`, `class_declaration`, `interface_declaration`, `method_definition` | Cortar antes del `statement_block` que abre `{` |
| Python | `function_definition`, `class_definition` | Recortar el cuerpo indentado después del `:` |
| C, C++, C# | `function_definition`, `struct_specifier`, `class_specifier` | Igual que Rust |
| Otros | Fallback | Sin parser: devolver `Vec::new()` |

3. Implementar primero Rust, TypeScript, Python. El resto como fallback.

4. La lógica de corte usa el AST: para un nodo declarativo, se busca el hijo que representa el cuerpo (`body`, `block`, `declaration_list`) y se toma el texto fuente desde `start_byte` del nodo hasta `start_byte` del hijo cuerpo (excluyendo el cuerpo).

#### En `src/mcp/handler.rs`

5. Agregar struct de parámetros:
   ```rust
   #[derive(Debug, Deserialize, JsonSchema)]
   pub struct VecOutlineParams {
       pub file_path: String,
   }
   ```

6. Agregar handler `vec_outline` con tool macro, misma validación de path que `vec_read_lines` (dentro del proyecto, tamaño máximo).

7. **Formato de salida:**
   ```
   Outline of src/cli/install.rs (rust):
     L10  pub struct InstallArgs
     L18  pub enum AgentTarget
     L73  const MCP_INSTRUCTIONS
     L116 fn opencode_config_dir() -> Option<PathBuf>
     L140 impl AgentTarget
     L224 pub fn execute(args: &InstallArgs, project_path: &Path) -> Result<()>
   ```

#### En `src/engine/mod.rs`

8. Declarar `pub mod outliner;`

### Tests a agregar

- `outline_rust_file_with_structs_and_fns`
- `outline_typescript_file_with_classes_and_interfaces`
- `outline_python_file_with_class_and_defs`
- `outline_unknown_language_returns_empty`
- `vec_outline_rejects_path_outside_project`
- `vec_outline_file_not_found`

**Esfuerzo:** ~200-300 líneas, 4-8 horas.

**Riesgo:** La extracción de firmas por gramática de tree-sitter. Cada lenguaje organiza los nodos distinto. Empezar por Rust y expandir.

---

## Resumen

| # | Punto | Archivos | Esfuerzo |
|---|-------|----------|----------|
| 1.3 | Anti-Ansiedad | `install.rs`, `handler.rs` | 5 min |
| 1.2 | path_filter SQL | `vectors.rs`, `searcher.rs` | 1 h |
| 1.1 | vec_outline | 4 files (1 nuevo) | 4-8 h |
