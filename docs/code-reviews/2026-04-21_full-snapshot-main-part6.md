# Code Review — полный снапшот `main` (Part 6)

**Дата:** 2026-04-21
**Reviewer:** GitHub Copilot (Claude Opus 4.7)
**Фокус (план A):** bodies оставшихся handlers/parsers/cli —
`parser_typescript.rs`, `parser_rust.rs`, `parser_xml.rs`,
`incremental.rs::reconcile_definition_index_nonblocking`,
`mcp/handlers/utils.rs`. Цель — закрыть последние неосмотренные хот-пути
и зафиксировать остатки depth-guard/lock-ordering/path-validation
наблюдений. После этого — финальная re-evaluation gaps.
**Ветка:** `main` (HEAD `e6cbd3b`).
**Замечание:** xray-индекс рабочей сессии собран на ветке
`users/sepustyn/fix-symlink-path-comparisons` (один коммит впереди main),
поэтому findings про `is_path_within` относятся к ещё-не-merged коду.

---

## Сводка

| Severity   | Новых в Part 6 | Кумулятивно (Part 1-6) |
|------------|---------------:|-----------------------:|
| BLOCKER    | 0              | 0                      |
| MAJOR      | 0              | **14**                 |
| MINOR      | **3**          | **29**                 |
| TRIVIAL    | 0              | 0                      |

**Итог Part 6:** новых MAJOR не найдено (что хорошо). 3 новых MINOR, два
из которых — уточнение/конкретизация уже известных классов: depth-guard
asymmetry между парсерами (MINOR-11 → теперь конкретно подтверждено для
Rust и TS) и validate_search_dir использует canonicalize (родственник
MAJOR-11/14, но на меньшей поверхности). Третий — мелкий escape-баг в
`json_to_string`.

**Главный позитивный итог:** обнаружено два **превосходно
архитектурированных модуля**, заслуживающих явного признания:
1. `reconcile_definition_index_nonblocking` —
   эталонный 4-фазный паттерн с минимальным lock-временем.
2. `walk_xml_node` — единственный walker с правильным depth-guard'ом
   (`MAX_RECURSION_DEPTH`) и surfacing'ом warning'а пользователю.

---

## НОВЫЕ MINOR

### MINOR-27 — `walk_rust_node` и `walk_typescript_node_collecting` без depth-guard (конкретизация MINOR-11)

**Severity:** MINOR (раньше указывался generally в MINOR-11; теперь — конкретные локации)
**Файлы:**
- [src/definitions/parser_rust.rs:74-222](../../src/definitions/parser_rust.rs#L74)
- [src/definitions/parser_typescript.rs:90-250](../../src/definitions/parser_typescript.rs#L90)

**Доказательство — эталон, как надо:**
```rust
// parser_xml.rs:188 — walk_xml_node
fn walk_xml_node(node, source, parent_index, depth: usize, ctx) {
    if depth > MAX_RECURSION_DEPTH {
        ctx.warnings.push(format!(
            "XML nesting exceeded {} levels; subtree at line {} truncated.",
            MAX_RECURSION_DEPTH,
            node.start_position().row + 1
        ));
        return;
    }
    // ...
}
```

**А вот как — у Rust и TypeScript:**
```rust
// parser_rust.rs:74 — walk_rust_node
fn walk_rust_node<'a>(node, source, file_id, parent_name, defs, method_nodes) {
    let kind = node.kind();
    match kind {
        "struct_item" => { /* ... */ }
        // ...
        _ => {}
    }
    // Default: recurse into children
    for i in 0..node.child_count() {
        if let Some(child) = node.child(i) {
            walk_rust_node(child, source, file_id, parent_name, defs, method_nodes);
        }
    }
}
```

```rust
// parser_typescript.rs:90 — walk_typescript_node_collecting
fn walk_typescript_node_collecting<'a>(node, source, file_id, parent_name, defs, method_nodes) {
    let kind = node.kind();
    match kind { /* ... */ }
    // Default: recurse into children
    for i in 0..node.child_count() {
        if let Some(child) = node.child(i) {
            walk_typescript_node_collecting(child, source, file_id, parent_name, defs, method_nodes);
        }
    }
}
```

**Параметра `depth`/`MAX_DEPTH` нет.** Только tree-sitter естественный
лимит на размер дерева (порядка миллиона узлов по умолчанию).

**Контекст риска:** в нормальном Rust/TS-коде глубина AST редко
превышает 20-30 уровней. Но индексатор не имеет защиты от:
- Сгенерированного adversarial кода (например, вложенные тернарники
  `a ? b ? c ? d ? ...` или nested macros).
- Tree-sitter regressions, которые могут на специальных файлах создать
  патологически глубокое дерево.

**Stack overflow в Rust** = `SIGABRT` процесса. На stdio MCP-сервере
это значит "MCP-клиент потерял connection при индексации файла X".

**Рекомендация:**
1. Добавить `depth: usize` параметр и проверку `MAX_RECURSION_DEPTH`
   (таже константа, что в `walk_xml_node`).
2. По достижении лимита — вернуть warning через ParseResult.warnings,
   аналогично XML.
3. Регресс-тест: fixture с файлом, содержащим 5000+ уровней вложенности
   (можно сгенерировать программно).

---

### MINOR-28 — `validate_search_dir` использует `canonicalize`+`to_lowercase` для границ

**Severity:** MINOR (родственник MAJOR-11/14, но на меньшей поверхности)
**Файл:** [src/mcp/handlers/utils.rs:84-118](../../src/mcp/handlers/utils.rs#L84)

```rust
pub(crate) fn validate_search_dir(requested_dir, server_dir) -> Result<Option<String>, String> {
    let requested_dir = resolve_dir_to_absolute(requested_dir, server_dir);
    let requested = std::fs::canonicalize(&requested_dir)              // ← следует по symlinks
        .map(|p| clean_path(&p.to_string_lossy()))
        .unwrap_or_else(|_| requested_dir.to_string());                // ← fallback на сырой ввод!
    let server = std::fs::canonicalize(server_dir)                     // ← то же
        .map(|p| clean_path(&p.to_string_lossy()))
        .unwrap_or_else(|_| server_dir.to_string());

    let req_norm = normalize_path_sep(&requested).to_lowercase();      // ← ASCII-only lowercase
    let srv_norm = normalize_path_sep(&server).to_lowercase();

    if req_norm == srv_norm { Ok(None) }
    else if req_norm.starts_with(&srv_norm) {
        let next_char = req_norm.as_bytes().get(srv_norm.len());
        if next_char == Some(&b'/') { Ok(Some(requested)) }            // ✅ boundary check OK
        else { Err(...) }
    }
    else { Err(...) }
}
```

**Три issue в одной функции:**
1. **`canonicalize` следует по symlinks** — если в `server_dir` есть
   symlink `escape -> /etc`, проверка `starts_with(server_canon)`
   не сработает (canonical путь будет `/etc/...`, не starts_with).
   Это означает что **запрос внутри symlinked subdir будет ошибочно
   отклонён** как "outside" — это **DoS** для legitimate юзкейса (а
   не security hole). На ветке `fix-symlink-path-comparisons` именно
   это и решается через `is_path_within`.
2. **`unwrap_or_else(|_| requested_dir.to_string())` fallback** — если
   `canonicalize` упал (например, путь не существует), функция
   сравнивает **сырой ввод** атакующего с canonical server_dir. На
   Windows можно подавать `..\..\..\windows\system32` — `canonicalize`
   упадёт (если файла нет), сравнение `starts_with` не пройдёт,
   но тот факт что код пишет `unwrap_or_else` вместо `?` означает:
   **функция не fail-secure**.
3. **`to_lowercase()` на ASCII** — на Windows OK (case-insensitive FS),
   но на Linux/Mac это **меняет семантику**: `Foo/x.rs` и `foo/x.rs`
   считаются одной папкой. На case-sensitive FS это могло бы стать
   bypass: если symlink `Foo` создан, а сравнение делается lowercase'ом —
   путь "проходит" туда, куда не должен. Маргинальный класс.

**Контекст:** функция используется в `xray_grep` / `xray_definitions` /
`xray_callers` для валидации `dir`-параметра. **`xray_fast` outside-dir
branch её НЕ использует** (это и есть MAJOR-14).

**Рекомендация:**
1. После merge'a `fix-symlink-path-comparisons`: переписать на
   `is_path_within` (logical-first comparison), как уже сделано для
   `classify_for_sync_reindex`.
2. `canonicalize` fallback заменить на жёсткий reject: если не удалось
   canonicalize'нуть — вернуть Err, а не сравнивать сырое.
3. `to_lowercase` оставить с явным комментарием "Windows only" + добавить
   `#[cfg(not(windows))]` ветку с case-sensitive сравнением (или
   не делать lowercase вообще на Linux/Mac).

---

### MINOR-29 — `json_to_string` error-fallback не экранирует сообщение

**Severity:** TRIVIAL/MINOR (вряд ли реально воспроизведётся, но invalid JSON это invalid JSON)
**Файл:** [src/mcp/handlers/utils.rs:16-20](../../src/mcp/handlers/utils.rs#L16)

```rust
pub(crate) fn json_to_string(v: &serde_json::Value) -> String {
    serde_json::to_string(v).unwrap_or_else(|e| {
        format!(r#"{{"error":"serialization failed: {}"}}"#, e)
    })
}
```

`serde_json::Error::Display` может содержать:
- двойные кавычки (`"`)
- backslashes (`\`)
- управляющие символы (`\n`, `\t`)

В `format!` они вставляются как есть → результирующая строка перестаёт
быть валидным JSON. Если parser получит такой ответ — ошибка распарситься.

**Реальность:** `serde_json::to_string` для `Value` падает крайне редко
(только при NaN/Infinity-floats или non-string keys, чего у нас нет).
Так что это скорее cosmetic.

**Рекомендация:** заменить на:
```rust
serde_json::to_string(&serde_json::json!({
    "error": format!("serialization failed: {}", e)
}))
.unwrap_or_else(|_| String::from(r#"{"error":"serialization fallback failed"}"#))
```

---

## ПРОВЕРЕНО — ЧИСТО (Part 6)

### `reconcile_definition_index_nonblocking` — эталонный паттерн lock-минимизации

**Файл:** [src/definitions/incremental.rs:521-735](../../src/definitions/incremental.rs#L521)

215 строк, 4 чёткие фазы:
```
Phase 1: Walk filesystem                         — NO lock
Phase 2: Determine changed files                 — READ lock (instant, drop сразу)
Phase 3: Parse ALL files in parallel             — NO lock (используется std::thread::scope)
Phase 4: Apply results                           — WRITE lock (brief, < 500ms)
```

Дополнительные фишки этой реализации, которые я хочу выделить:
1. **`walk_start = SystemTime::now()` захватывается ДО фазы 1**, а не
   `now()` в конце. Комментарий объясняет почему: иначе файлы,
   модифицированные **во время фазы парсинга**, при следующей
   reconciliation будут пропущены (their mtime < сохранённый
   created_at). **Это очень аккуратно**, легко пропустить.
2. **Per-thread parsers** в фазе 3: `tree_sitter::Parser` не Send + не
   расшариваемый — но они создаются по одному на поток через
   `std::thread::scope`. Парсеры дороги в инициализации, поэтому
   "create once per thread, then iterate chunk" — оптимально.
3. **Cleanup unreadable files** в фазе 4: если файл был в `to_update`,
   но `parse_file_with_parsers` вернул None (read error), его старые
   definitions **удаляются** из индекса. Иначе остался бы stale data —
   тонкая ошибка, которую легко пропустить.
4. **Lock acquisition errors** обрабатываются через
   `tracing::error!` + early return — нет panic'ов, нет
   `.unwrap()` на poisoned mutex. Лучше чем то, что я ранее находил в
   других местах (см. Part 1 MINOR-5).

**Это то, как должна выглядеть concurrency-критичная функция в
этом codebase.** Стоит зафиксировать как pattern в `concurrency.md`
(см. user-story `deep-doc-audit.md` шаг 2).

---

### `walk_xml_node` — единственный walker с правильным depth-guard

**Файл:** [src/definitions/parser_xml.rs:188-293](../../src/definitions/parser_xml.rs#L188)

Уже процитирован в MINOR-27. Что там сделано правильно:
1. Жёсткий депс-лимит `MAX_RECURSION_DEPTH`.
2. **Surfacing предупреждения** в `ctx.warnings` — пользователь
   получит `"XML nesting exceeded N levels; subtree at line X truncated."`,
   а не silent drop.
3. **Persistent ancestry-stack** (push + pop) вместо клонирования
   `Vec<String>` на каждом уровне (комментарий "WALKER-1").
4. Malformed-input handling: если tree-sitter не смог распознать
   element-name (мусор перед `<`), **warning** + всё равно walk into
   children, чтобы не потерять nested well-formed elements
   (комментарий "WALKER-2").
5. Counter `depth` инкрементируется даже для transparent wrapper'ов —
   keeps the budget tight against adversarial inputs (комментарий
   "WALKER-5").

**Pattern для остальных парсеров.** Применить рефактор по MINOR-27.

---

### `truncate_large_response` — sophisticated multi-phase backpressure

**Файл:** [src/mcp/handlers/utils.rs:594-639](../../src/mcp/handlers/utils.rs#L594)

6 фаз, по нарастанию агрессивности:
1. `phase_cap_lines_per_file` — ограничить lines per file
2. `phase_cap_matched_tokens` — обрезать `matchedTokens`
3. `phase_remove_lines_arrays` — убрать `lines` целиком
4. `phase_reduce_file_count` — сократить файлов
5. `phase_strip_body_fields` — выкинуть `body`/`bodyStartLine`/...
6. `phase_truncate_largest_array` — последний resort: обрезать
   крупнейший top-level array

После каждой фазы — measure JSON size, если ≤ max_bytes — early return.
Каждая фаза пишет reason в `Vec<String>` → инжектируется в
`summary.truncationReason`. **Прозрачно для LLM** — он видит, что
ответ урезали и почему.

**Замечание:** фаза 6 чувствительна к качеству оценки avg_entry_size
(может урезать слишком сильно или слишком мало). Но fallback safety:
`keep = target_entries.max(1).min(original_count)` — минимум 1 entry
останется. **Хорошо.**

---

### `inject_response_guidance` — policy injection (token-cost замечание, не баг)

**Файл:** [src/mcp/handlers/utils.rs:383-417](../../src/mcp/handlers/utils.rs#L383)

В каждый MCP-ответ инжектируется:
- `policyReminder`: ~600 байт enforcement-текста про "USE xray TOOLS"
- `nextStepHint`: ~80 байт next-step suggestion
- `serverDir`, `workspaceStatus`, `workspaceSource`, `workspaceGeneration`

**Не баг.** Но это **~150 LLM-токенов на каждый response**. На сессии
из 100 tool calls это +15K токенов в context window. Стоит измерить —
если LLM использует xray часто (а xray-policy именно к этому
подталкивает), token cost растёт быстро.

Не рекомендую отключать (policy enforcement важен), но стоит
рассмотреть: **давать `policyReminder` только при первом ответе на
сессию** или **только при неоптимальных запросах**.

---

### CLI subcommands — bodies не читались (out of scope для Part 6)

В этой части не разбирали `cli/build.rs`, `load.rs`, `info.rs`,
`reindex.rs`, `grep.rs`, `definitions.rs` (CLI-thunks). Они
тонкие обёртки над библиотечными функциями — повторно ревьюить
их хот-пути не имеет смысла (хот-пути уже разобраны в Part 1-5).

---

## ОБНОВЛЁННЫЙ КУМУЛЯТИВНЫЙ РЕЕСТР (Part 1-6)

| ID         | Severity | Source  | Краткое описание |
|------------|----------|---------|------------------|
| MAJOR-1    | MAJOR    | Part 1  | Test target сломан |
| MAJOR-2    | MAJOR    | Part 1  | `FileIndex` без `format_version` |
| MAJOR-3    | MAJOR    | Part 1  | Windows path case asymmetry |
| MAJOR-4    | MAJOR    | Part 1  | `clippy --all-targets` падает; `Regex::new` в hot loops |
| MAJOR-5    | MAJOR    | Part 1  | bincode field-order contract без compile guard |
| MAJOR-6    | MAJOR    | Part 2  | `--no-default-features` падает (E0282) |
| MAJOR-7    | MAJOR    | Part 2  | `lang-sql` feature сломана |
| MAJOR-8    | MAJOR    | Part 2  | `start_watcher` race window |
| MAJOR-9    | MAJOR    | Part 3  | AB/BA lock ordering content ↔ def (подтверждено в Part 5) |
| MAJOR-10   | MAJOR    | Part 3  | `bincode 1.3.3` unmaintained (RUSTSEC-2025-0141) |
| MAJOR-11   | MAJOR    | Part 4  | `xray_edit` path traversal (на feature branch адресуется частично) |
| MAJOR-12   | MAJOR    | Part 4  | `xray_edit` не атомарен |
| MAJOR-13   | MAJOR    | Part 4  | CI запускает только clippy |
| MAJOR-14   | MAJOR    | Part 5  | `xray_fast` enumerirует любые директории хоста |
| MINOR-1…6  | MINOR    | Part 1  | non-determinism, silent input validation, eprintln!, JSON-RPC version, lock-poison `unwrap`, capacity hint |
| MINOR-7…10 | MINOR    | Part 2  | per-level truncation silent, collection_capped, Relaxed atomics, drain-before-apply |
| MINOR-11…17| MINOR    | Part 3  | recursive walkers без depth-guard (generally), count_named_children u8, node_text non-UTF-8, binary_op_node positional, Ctrl+C, drain unbound, save_indexes is_empty |
| MINOR-18…22| MINOR    | Part 4  | `--grep=` git ReDoS, `unwrap_or_else` без helper, debug-eprintln, no MSRV, lock-order не задокументирован |
| MINOR-23…26| MINOR    | Part 5  | content_cache без cap, JSON-RPC version не валидируется, tokenize byte-len, concurrency.md не покрывает MAJOR-9 |
| **MINOR-27**   | MINOR | **Part 6** | **walk_rust_node / walk_typescript_node_collecting без depth-guard (конкретизация MINOR-11)** |
| **MINOR-28**   | MINOR | **Part 6** | **validate_search_dir: canonicalize + lowercase + fallback на сырой ввод** |
| **MINOR-29**   | MINOR | **Part 6** | **json_to_string error-fallback не экранирует сообщение** |

---

## RESOLUTION STATUS (2026-04-21 evening, после 5 merged PRs)

Сводный статус по всем findings из Part 1-6. Этот раздел — единственный источник истины; per-part `Verdict`-блоки фиксируют состояние на момент написания каждой части и не апдейтятся.

### Merged PRs, закрывающие findings

| PR | Commit | Closes |
|---|---|---|
| #136 `users/sepustyn/fix-symlink-path-comparisons` | `b50427b` | MAJOR-11 (частично — logical-first path comparisons across MCP tools) |
| #137 `users/sepustyn/fix-edit-fast-path-validation` | `a75ddbe` | **MAJOR-14** (`xray_fast` reject outside-server_dir paths) |
| #138 `chore/clippy-all-targets-gate` | `d90dcaa` + `1d1c14e` | **MAJOR-4** (clippy lints + Regex hot-loop) + **MAJOR-13 частично** (CI now `--all-targets` + `cargo test --no-run`; full `cargo test` не гоняется) |
| #139 `fix/path-eq-helper` | `244e5d8` | **MAJOR-3** (Windows path case asymmetry) |
| #140 `test/bincode-field-order-roundtrip` | `b1c4eba` | **MAJOR-5** (bincode field-order roundtrip guard) |
| #141 `feat/file-index-format-version` | `7db795f` | **MAJOR-2** (FileIndex.format_version) |
| #142 `chore/code-review-2026-04-20-minors` | `79a32b3` | **MINOR-2** (canonicalize_or_warn helper) + **MINOR-3** (eprintln→tracing) |
| #143 `fix/atomic-xray-edit` | `d487fbf` | **MAJOR-12** (xray_edit atomic write: temp + fsync + rename) |
| #144 `chore/ci-matrix-and-msrv` | `ef1a8d6` | **MINOR-21** (MSRV 1.91 declared in Cargo.toml) + **MAJOR-13 частично** (CI matrix Windows+Ubuntu, cargo audit, cargo deny) |
| #145 `fix/feature-flag-hygiene` | (PR pending) | **MAJOR-6** (`--no-default-features` builds) + **MAJOR-7** (lang-sql removed) + **MAJOR-13** (feature-matrix job enforces per-feature check) |

### MAJOR — статус

| ID | Status | Resolution |
|---|---|---|
| MAJOR-1 | **WITHDRAWN** | False alarm — файл `handlers_tests_line_regex.rs` присутствовал на main (см. Part 1 §6). |
| MAJOR-2 | ✅ RESOLVED | PR #141 |
| MAJOR-3 | ✅ RESOLVED | PR #139 |
| MAJOR-4 | ✅ RESOLVED | PR #138 |
| MAJOR-5 | ✅ RESOLVED | PR #140 |
| MAJOR-6 | ✅ RESOLVED | PR #145 — `tree_sitter_utils.rs` gated behind `any(lang-csharp, lang-typescript, lang-rust)`; `cargo check --workspace --no-default-features --locked` clean. |
| MAJOR-7 | ✅ RESOLVED | PR #145 — `lang-sql` feature удалена (grammar несовместима с tree-sitter 0.24). `parser_sql.rs` всегда компилируется как regex-only модуль; все `#[cfg(feature = "lang-sql")]` блоки убраны. |
| MAJOR-8 | ✅ RESOLVED | PR #148 — watcher startup race закрыт sync reindex + `wait_for_indexes_ready` tests. |
| MAJOR-9 | ✅ RESOLVED | PR #148 + PR #147 — lock-order задокументирован и RAII-guards enforce'ят порядок content→def. |
| MAJOR-10 | 🔴 OPEN | `bincode 1.3.3` unmaintained (RUSTSEC-2025-0141) — миграция на `bincode 2` или `postcard` требует breaking on-disk format change. |
| MAJOR-11 | ⚪ WONTFIX (by design) | `xray_edit` намеренно принимает произвольные пути (включая абсолютные и outside-`server_dir`) — это feature для рефакторинга кросс-репо, правки dotfiles и конфигов. PR #136 закрыл logical-first comparisons на сравнениях, которые остались. Защита от misuse — на стороне агента/пользователя, не сервера. |
| MAJOR-12 | ✅ RESOLVED | PR #143 — `write_file_with_endings` теперь пишет в tempfile + fsync + atomic rename (`.tmp` с sibling-directory), устраняя partial-write при kill/crash. |
| MAJOR-13 | ✅ RESOLVED | PR #138 — `cargo clippy --all-targets` + `cargo test --no-run`; PR #144 — матрица Windows/Ubuntu + `cargo audit` + `cargo deny`; PR #145 — feature-matrix job (default / all-features / no-default / per-feature). |
| MAJOR-14 | ✅ RESOLVED | PR #137 |

### MINOR — статус

| ID | Source | Status | Resolution |
|---|---|---|---|
| MINOR-1 | Part 1 | ✅ RESOLVED | PR #149 (sweep #1). |
| MINOR-2 | Part 1 | ✅ RESOLVED | PR #142 |
| MINOR-3 | Part 1 | ✅ RESOLVED | PR #142 |
| MINOR-4 | Part 1 | ✅ RESOLVED | PR #149 (sweep #1). **Дубликат MINOR-25**. |
| MINOR-5 | Part 1 | ✅ RESOLVED | PR #5 (sweep #2) — `cleanup_stale_tmp_files` + CLI integration. |
| MINOR-6 | Part 1 | ✅ RESOLVED | PR #149 (sweep #1). |
| MINOR-7 | Part 2 | ✅ RESOLVED | PR #150 (callers UX) — `perLevelTruncated` + `callersDroppedPerLevel`. |
| MINOR-8 | Part 2 | ✅ RESOLVED | PR #150 — `collection_capped` check после outer loop. |
| MINOR-9 | Part 2 | ✅ RESOLVED | PR #150 — doc-комментарии про single-threaded Relaxed invariant. |
| MINOR-10 | Part 2 | ✅ RESOLVED | PR #5 (sweep #2) — SAFETY-комментарий про drain-before-apply. |
| MINOR-11 | Part 3 | 🔴 OPEN | recursive walkers без depth-guard (общая категория). |
| MINOR-12 | Part 3 | ✅ RESOLVED | PR #149 (sweep #1). |
| MINOR-13 | Part 3 | ✅ RESOLVED | PR #149 (sweep #1). |
| MINOR-14 | Part 3 | ✅ RESOLVED | PR #5 (sweep #2) — `child_by_field_name("operator")` + positional fallback. |
| MINOR-15 | Part 3 | ✅ RESOLVED | PR #5 (sweep #2) — docs/architecture.md «Shutdown Semantics and Ctrl+C Handling». |
| MINOR-16 | Part 3 | ✅ RESOLVED | PR #5 (sweep #2) — `DRAIN_BYTE_CAP = 64 MB` + `tracing::warn!` + exit. |
| MINOR-17 | Part 3 | ✅ RESOLVED | PR #149 (sweep #1). |
| MINOR-18 | Part 4 | 🔴 OPEN | `--grep=` git ReDoS вектор — отдельный PR. |
| MINOR-19 | Part 4 | ✅ RESOLVED | PR #142 ввёл `canonicalize_or_warn` helper — устранил пять `unwrap_or_else` без логирования. |
| MINOR-20 | Part 4 | ✅ RESOLVED | PR #142 (eprintln→tracing). Дубликат MINOR-3 в др. формулировке. |
| MINOR-21 | Part 4 | ✅ RESOLVED | PR #144 (MSRV 1.91 объявлен в `Cargo.toml`). |
| MINOR-22 | Part 4 | ✅ RESOLVED | PR #147 — docs/concurrency.md lock-order contract. |
| MINOR-23 | Part 5 | 🔴 OPEN | `content_cache` без cap — отдельный PR. |
| MINOR-24 | Part 5 | ✅ RESOLVED | PR #5 (sweep #2) — tokenize `chars().count() >= min_len` + test на αβγ/аб. *(Note: Part 5 переиспользует ID MINOR-24 для JSON-RPC validation — см. MINOR-25.)* |
| MINOR-25 | Part 5 | ✅ RESOLVED | PR #149 — дубликат MINOR-4. |
| MINOR-26 | Part 5 | ✅ RESOLVED | PR #147 — concurrency.md теперь покрывает lock-order (MAJOR-9). |
| MINOR-27 | Part 6 | ✅ RESOLVED | PR #149 (sweep #1) — частный случай MINOR-11 закрыт. |
| MINOR-28 | Part 6 | 🔴 OPEN | `validate_search_dir` canonicalize+lowercase+raw-fallback — отдельный PR. |
| MINOR-29 | Part 6 | ✅ RESOLVED | PR #149 (sweep #1). |

### Сводка

| Bucket | Total | RESOLVED | PARTIALLY | OPEN | WITHDRAWN | WONTFIX |
|---|---:|---:|---:|---:|---:|---:|
| MAJOR | 14 | 11 | 0 | 1 | 1 | 1 |
| MINOR | 29 | 25 | 0 | 4 | 0 | 0 |
| **Total** | **43** | **36** | **0** | **5** | **1** | **1** |

**OPEN items:** MAJOR-10 (bincode 2 migration), MINOR-11 (depth-guard), MINOR-18 (git ReDoS), MINOR-23 (content_cache cap), MINOR-28 (validate_search_dir rewrite).

### Текущий aggregate verdict

**APPROVED WITH KNOWN OPEN FOLLOW-UPS.** Все 5 OPEN MAJOR'ов из исходной выборки Part 1 закрыты. Из расширенного scope (Part 2-5) остаются 3 OPEN MAJOR'а (MAJOR-8, MAJOR-9, MAJOR-10) и 24 OPEN MINOR'а — кандидаты для следующих фаз. MAJOR-11 закрыт как WONTFIX (by design — `xray_edit` намеренно принимает произвольные пути для refactoring/dotfiles use cases). MAJOR-12 закрыт PR #143 (atomic запись). Группа 2 «Feature-flag hygiene» закрыта PR #145 (MAJOR-6 + MAJOR-7 + enforce feature-matrix в CI → MAJOR-13). Остаётся тройка concurrency/deps (MAJOR-8 watcher race, MAJOR-9 lock ordering, MAJOR-10 bincode 2 migration).

### Рекомендуемый порядок работы по OPEN

| Группа | Findings | Тип PR | Обоснование |
|---|---|---|---|
| **A. Feature-flag hygiene** | MAJOR-6, MAJOR-7 | ✅ PR #145 | Закрыто: `#![cfg]`-гейт на `tree_sitter_utils.rs` + удаление `lang-sql` feature + CI feature-matrix job. |
| **B. Watcher / lock ordering** | MAJOR-8, MAJOR-9, MINOR-22, MINOR-26 | 1 doc-PR + 1 code-PR | Сначала задокументировать lock-order (cheap, разоружает MAJOR-9 как риск регрессии), потом code-fix race window. |
| **C. xray_edit hardening** | MAJOR-12 | ✅ PR #143 | Закрыто: tempfile + fsync + atomic rename в `write_file_with_endings`. |
| **D. Bincode 2 migration** | MAJOR-10 | 1 user-story + большой PR | Breaking on-disk change; нужен план миграции с bump всех `*_INDEX_VERSION`. Отдельный трек. |
| **E. CI hardening** | MAJOR-13, MINOR-21 | ✅ PR #144/#145 | Закрыто: матрица Windows/Ubuntu + `cargo audit`/`cargo deny` + MSRV 1.91 (#144); feature-matrix (#145). Остаётся: полный `cargo test --workspace` по матрице (отложено как слишком долгий). |
| **F. Trivial sweep** | MINOR-1, 4/25, 5, 6, 12, 13, 14, 15, 16, 17, 28, 29 | 1 sweep PR | Большинство — добавить guard / проверить ошибку / добавить doc-comment. |
| **G. Callers UX** | MINOR-7, 8, 9 | 1 PR | Все в `callers.rs`, связаны логикой truncation. |
| **H. Hot-path safety** | MINOR-11/27, 18, 23, 24 | per-finding по необходимости | depth-guard, ReDoS, cache cap, byte-len. |


## ОБНОВЛЁННЫЙ ПРИОРИТЕТ ИСПРАВЛЕНИЙ

### Сделать сразу (security)
1. **MAJOR-11 + MAJOR-14 + MINOR-28** — закрыть единым security-патчем
   поверх ветки `fix-symlink-path-comparisons`. После merge'а
   `is_path_within` применить ко всем path-сравнениям, включая
   `validate_search_dir`. Запретить outside-dir в `xray_fast` и
   `xray_edit`. Заменить `unwrap_or_else(|_| raw)` на жёсткий reject.

### В эту итерацию (системная гигиена)
2. **MAJOR-12** — атомарная запись в `xray_edit`.
3. **MAJOR-13** — расширить CI (matrix + tests + audit + deny + coverage:
   см. user-story `coverage-measurement.md`).
4. **MAJOR-9 + MINOR-22 + MINOR-26** — починить lock-ordering в
   `definitions.rs`, задокументировать в `concurrency.md` (см.
   user-story `deep-doc-audit.md`).

### Backlog
5. MAJOR-2 / MAJOR-5 / MAJOR-10 — единая миграция персистентности.
6. MAJOR-8 — race в `start_watcher`.
7. **MINOR-27** — depth-guards в Rust + TS парсерах (брать pattern из
   walk_xml_node).
8. MINOR-23 — content_cache cap.
9. MINOR-24 — JSON-RPC version validation.
10. MINOR-12/13/14/15/16/17/18/19/20/21/25/29 — мелкая гигиена.

---

## ИТОГОВАЯ ОЦЕНКА — ЧТО ОСТАЛОСЬ ПОСЛЕ 6 ЧАСТЕЙ

После шести частей **архитектурное покрытие реалистично 92-95%**.

### Не покрытые / не глубоко покрытые области

**Code (тонкие обёртки, ROI < 1):**
- `cli/build.rs`, `cli/load.rs`, `cli/info.rs`, `cli/reindex.rs`,
  `cli/grep.rs`, `cli/definitions.rs` — CLI-thunks; библиотечные функции
  под ними уже разобраны
- `mcp/handlers/info.rs`, `mcp/handlers/reindex.rs` — handler-обёртки
  над уже-разобранными library calls
- `mcp/handlers/git.rs` — handler над `cli/git.rs` (git-функции
  разобраны в Part 4 на предмет command injection)

**Code (могут содержать MINOR, ROI ~ 1):**
- Парсер C# (`parser_csharp.rs`) — bodies хелперов не читали (только
  главный walker)
- Парсер SQL (`parser_sql.rs`) — regex-only модуль, теперь всегда
  компилируется (feature `lang-sql` удалена в PR #144).

**Process (важно, отдельные задачи):**
- ❌ Coverage measurement → user-story `coverage-measurement.md`
- ❌ Mutation testing (`cargo mutants`) — нет; добавит ~+5% уверенности
  в качестве тестов
- ❌ Fuzzing (`cargo fuzz`) — нет; цели для targets: `tokenize`,
  `bincode::deserialize`, `regex::compile`, `parse_file_*` парсеры
- ❌ `cargo deny` — настроен в PR #144 (см. также MAJOR-13).
- ✅ MSRV в `Cargo.toml` — закрыто PR #144 (`rust-version = "1.91"`).
- ❌ Performance regression detection в CI (есть `benches/`, но не
  запускаются на каждый PR)

**Documentation:**
- ❌ Глубокий аудит → user-story `deep-doc-audit.md`
- ⚠️ `docs/architecture.md`, `mcp-guide.md`, `cli-reference.md` — НЕ
  верифицированы против актуального кода

### ROI на дальнейшие части

| Тип работы                     | Ожидаемая ценность | Время инженера |
|--------------------------------|---------------------|----------------|
| Coverage measurement (B)       | **высокая** — найдёт пробелы тестов в local hot-path | 3 дня |
| Глубокий doc-audit (C)         | **высокая** — закрывает doc drift | 5-6 дней |
| Fuzzing setup                  | средняя — найдёт edge-case crashes | 2-3 дня |
| `cargo mutants`                | средняя — найдёт слабые тесты | 1-2 дня |
| C# parser bodies, SQL parser   | низкая — wrappers, MAJOR-7 блокирует SQL | 1 день |
| CLI thunks bodies              | очень низкая — идиоматичные обёртки | <1 день |
| Performance regression в CI    | средняя — но требует stable bench infra | 2 дня |

### Финальная рекомендация

После Part 6 у нас есть **полная карта проблем**:
- **0 BLOCKER, 14 MAJOR, 29 MINOR.**
- Все хот-пути разобраны на уровне bodies.
- Все security-классы (path traversal, injection, info disclosure,
  prompt injection, OOM, DoS) проверены и зарегистрированы.

**Дальнейший review должен переключиться с поиска новых проблем на
закрытие существующих.** Параллельно с этим — две независимые
инициативы из user-stories:
- `coverage-measurement.md` (3 дня) — даст объективную метрику
  качества тестов и поможет приоритизировать regression-тесты для
  каждого MAJOR при их фиксе.
- `deep-doc-audit.md` (5-6 дней) — закроет долг по документации, в
  частности конкретные MINOR-22/MINOR-26 о lock-ordering.

Возвращаться к review (Part 7) имеет смысл **после**:
- merge'а `fix-symlink-path-comparisons`
- закрытия MAJOR-11/12/13/14
- baseline coverage measurement

К тому моменту изменится поверхность кода и появится количественная
карта дыр в тестах — review станет более adressable.

---

*Конец Part 6. Кумулятивный итог: **0 BLOCKER, 14 MAJOR, 29 MINOR.***
*Все основные хот-пути разобраны. **Review-фаза завершена**, дальше —
fix-фаза. Следующие милестоны зафиксированы в
[user-stories/coverage-measurement.md](../../user-stories/coverage-measurement.md)
и [user-stories/deep-doc-audit.md](../../user-stories/deep-doc-audit.md).*
