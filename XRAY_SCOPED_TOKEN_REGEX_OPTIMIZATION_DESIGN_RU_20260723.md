# Xray MCP: дизайн оптимизации scoped token-regex

## 1. Статус документа

- **Статус:** Implementation-ready design.
- **Дата:** 2026-07-23.
- **Целевой baseline:** состояние после scoped-query PR 1-3.
- **Основной компонент:** `xray_grep` с `regex=true`, `substring=false`.
- **Основной production corpus:** `C:\Repos\Shared`.
- **Изменение on-disk формата в обязательных этапах:** нет.
- **Reindex в обязательных этапах:** не требуется.
- **Persistent reverse-token IDs:** только условный третий этап после измерений.
- **Связанный документ:** `XRAY_SCOPED_QUERY_PERFORMANCE_DESIGN_RU_20260723.md`.

## 2. Резюме решения

После PR 1-3 Xray корректно ограничивает response token-regex и применяет file scope к postings, но порядок выполнения остается дорогим:

```text
parse request
  -> resolve file scope
  -> scan global token vocabulary
  -> clone all matching token strings
  -> sort + dedup expansion
  -> traverse global posting list каждого expanded token
  -> reject почти все postings по file scope
  -> score оставшиеся postings
```

Production-измерения на Shared показали:

- vocabulary содержит `3 912 392` unique tokens;
- broad regex `.*` раскрывает все `3 912 392` tokens;
- scoring проверяет `14 922 512` postings при любом file scope;
- для scope из одного файла полезны только `34` postings;
- posting waste равен `99.999772%`;
- median `searchTimeMs` для K=1 равен `4 204.59 ms`;
- exact-token запрос к тому же файлу занимает `65.75 ms`;
- response amplification уже исправлен: ответ broad regex занимает около `1.8 KiB`, а не сотни KiB.

Рекомендуется выполнить работу в три этапа:

1. **PR 4: empty-scope short-circuit и phase telemetry.**
   - Сохраняет текущую global expansion semantics.
   - Компилирует regex до short-circuit, чтобы invalid regex оставался ошибкой.
   - При пустом execution scope не сканирует vocabulary и postings.
   - Разделяет compile, vocabulary scan/collect, sort/dedup и posting score timings.

2. **PR 5: scoped expansion через существующий authoritative `file_tokens`.**
   - Для узкого non-empty scope строит query-local union tokens выбранных файлов.
   - Применяет regex только к этому token universe.
   - Сохраняет глобальный IDF и использует глобальные posting lists только для совпавших scoped tokens.
   - При Unavailable, RebuildPending, Inconsistent или слишком широком reverse map использует текущий global fallback.
   - Не меняет persisted schema.

3. **PR 6: compact reverse-token IDs, только условно.**
   - Начинается только если PR 5 докажет существенный выигрыш, но string-based `file_tokens` окажется слишком дорогим по RSS, startup или watcher latency.
   - Требует intern table, version bump, rebuild и отдельного storage/watcher design review.

Исходный условный PR про sorted postings не активируется автоматически. После PR 5 повторные измерения должны определить, остается ли global posting traversal dominant для небольшого scoped token universe.

## 3. Подтвержденная проблема

### 3.1. Production corpus

Shared snapshot во время измерения:

| Метрика | Значение |
|---|---:|
| Physical content-index slots | 82 019 |
| Unique tokens | 3 912 392 |
| Posting records | 14 922 512 |
| Target scope | 1 файл |
| Target file | `IB2BWorkflowsManager.cs` |

Physical slots включают stable-ID tombstones. Это не влияет на основной вывод: token vocabulary и posting traversal являются глобальными независимо от K.

### 3.2. Измерительная матрица

Каждый case выполнялся один cold раз и пять последовательных warm раз. Ниже приведены warm medians.

| Case | K | Regex | searchTimeMs | tokensExamined | matchedTokenCount | postingListsVisited | postingsChecked | postingsInScope |
|---|---:|---|---:|---:|---:|---:|---:|---:|
| A0 | 0 | impossible | 108.94 | 3 912 392 | 0 | 0 | 0 | 0 |
| A1 | 1 | impossible | 139.89 | 3 912 392 | 0 | 0 | 0 | 0 |
| Aall | all | impossible | 60.94 | 3 912 392 | 0 | 0 | 0 | 0 |
| B0 | 0 | `.*` | 4 672.66 | 3 912 392 | 3 912 392 | 3 912 392 | 14 922 512 | 0 |
| B1 | 1 | `.*` | 4 204.59 | 3 912 392 | 3 912 392 | 3 912 392 | 14 922 512 | 34 |
| Ball | all | `.*` | 10 347.55 | 3 912 392 | 3 912 392 | 3 912 392 | 14 922 512 | 14 922 512 |
| C | 1 | selective | 129.12 | 3 912 392 | 1 | 1 | 4 | 1 |
| D | 1 | exact token | 65.75 | n/a | n/a | n/a | n/a | n/a |

### 3.3. Выводы из измерений

1. Empty scope не short-circuit-ит token regex.
2. Vocabulary scan не зависит от K.
3. Broad expansion создает и сортирует миллионы cloned strings.
4. Posting traversal не зависит от K до проверки `scope.contains(file_id)`.
5. Для B1 полезны `34` из `14 922 512` postings.
6. Ball дороже B1, потому что Ball дополнительно materialize-ит и score-ит все postings, но B1 все равно платит полный traversal.
7. Selective regex C показывает, что global vocabulary scan сам по себе стоит примерно десятки или сотни ms, но не объясняет multi-second broad case.
8. Разность `B1 - A1` нельзя называть чистым posting time: она также содержит clone, collection, sort и dedup `3.9M` matching token strings.

### 3.4. Производные метрики

Для B1:

$$
postingWaste = 1 - \frac{34}{14\,922\,512}
             = 99.999772\%
$$

Отношение impossible vocabulary scan к broad request:

$$
expansionShareApprox = \frac{139.8932}{4204.5928}
                     \approx 3.327\%
$$

Это только приближение. A1 не выполняет collection/sort/dedup broad expansion, поэтому точные phase shares должны измеряться внутри production-кода или isolated benchmark.

## 4. Текущая архитектура

### 4.1. MCP call path

```text
xray_grep MCP dispatch
  -> handle_xray_grep
  -> parse args
  -> acquire content-index read lock
  -> resolve_grep_file_scope
  -> collect scope coverage
  -> raw_terms lowercase
  -> expand_regex_terms
       -> expand_regex_terms_inner(deduplicate=true)
       -> compile each pattern
       -> scan index.index.keys() for each pattern
       -> clone matching token strings
       -> sort + dedup
  -> score_normal_token_search
       -> lookup each expanded token posting list
       -> traverse every posting
       -> scope.contains(posting.file_id)
       -> global IDF + scoped scoring
  -> finalize_grep_results
  -> build_grep_response
  -> apply invert/scope telemetry/coverage/truncation
```

Scope уже разрешается до regex expansion, поэтому empty-scope и scoped-universe optimization не требуют нового path resolver.

### 4.2. CLI call path

CLI grep использует тот же низкоуровневый scanner, но намеренно вызывает duplicate-preserving expansion:

```text
Commands::Grep
  -> cmd_grep
  -> expand_grep_terms
  -> expand_regex_terms_preserving_duplicates
  -> expand_regex_terms_inner(deduplicate=false)
```

MCP сортирует и дедуплицирует совпадения overlapping patterns. CLI сохраняет дубликаты. Эта разница является существующим behavior contract и не должна случайно исчезнуть при рефакторинге compile/scan phases.

### 4.3. Текущая regex semantics

Каждый user pattern компилируется как:

```text
(?i)^<pattern>$
```

Следствия:

- matching case-insensitive;
- anchors относятся к отдельному indexed token;
- whitespace и source-line boundaries не поддерживаются;
- для source-line regex нужен `lineRegex=true`;
- invalid pattern возвращает MCP error до search response.

### 4.4. Текущий scoring

Для каждого expanded token:

1. Берется глобальный posting list.
2. `doc_freq = postings.len()`.
3. IDF рассчитывается по глобальному physical file-slot universe.
4. Каждый posting проверяется через resolved scope membership.
5. Только in-scope posting изменяет score/result.

Scoped expansion не должна менять `doc_freq`, `total_docs`, TF, IDF, ranking или occurrence accounting.

## 5. `file_tokens`: доступные данные и ограничения

### 5.1. Представление

`ContentIndex` уже содержит:

```rust
pub file_tokens: Vec<Vec<String>>,
pub file_tokens_authoritative: bool
```

Это reverse map:

```text
file_id -> unique token strings in that file
```

### 5.2. Persisted contract

Оба поля имеют `serde(skip)`:

- они не записываются в content index;
- load создает пустой map и `authoritative=false`;
- clone также сбрасывает reverse map;
- обычный read-only CLI load не платит memory cost reverse map;
- обязательные PR 4-5 не требуют schema bump.

### 5.3. Существующий lifecycle, который PR 5 сохраняет

Текущая реализация уже дает корректную publication boundary для query consumer-а, хотя имеет известный latency cost:

1. `build_watch_index_from` переводит index в watch policy, ставит `file_tokens_authoritative=true` и очищает reverse vector.
2. `schedule_rebuild_file_tokens` запускает O(total postings) rebuild.
3. Rebuild держит exclusive `ContentIndex` write lock и заполняет local slots внутри опубликованного index.
4. Query до начала rebuild может получить read lock, увидеть empty reverse vector и выбрать global fallback.
5. Query во время rebuild ожидает write lock.
6. Query после rebuild видит whole completed vector; partial intermediate slots не видны из-за write lock.
7. Первый watcher edit при failed background spawn выполняет существующий lazy rebuild под write lock до targeted mutation.
8. Add/update/remove поддерживают reverse slots, если maintenance policy authoritative.
9. Load/clone сбрасывают reverse map и policy в non-authoritative state.

Это означает:

- существующий lifecycle не дает false-negative только из-за фонового partial publish;
- pre-rebuild query корректно деградирует в global path;
- post-rebuild query может использовать scoped path;
- rebuild lock stall является pre-existing latency issue, но не создается PR 5;
- PR 5 не должен вызывать rebuild или менять startup/reindex publication.

### 5.4. Query-local eligibility contract

PR 5 добавляет только read-only eligibility helper:

```rust
enum ScopedFileTokensEligibility {
    Ready,
    Unavailable,
    RebuildPending,
    Inconsistent(ScopedFileTokensInvalidReason),
}

fn scoped_file_tokens_eligibility(
    index: &ContentIndex,
    scope: &ResolvedFileScope,
) -> ScopedFileTokensEligibility;
```

Правила:

```text
если file_tokens_authoritative == false:
    Unavailable
иначе если file_tokens.is_empty():
    RebuildPending
иначе если file_tokens.len() != files.len():
    Inconsistent(lengthMismatch)
иначе если любой scoped file_id вне range:
    Inconsistent(scopeIdOutOfRange)
иначе если scoped live file имеет positive token count и empty reverse slot:
    Inconsistent(emptyLiveSlot)
иначе:
    Ready
```

Planner использует scoped strategy только для `Ready`. Все остальные состояния используют current global vocabulary path и bounded fallback reason.

Eligibility helper не пытается доказать arbitrary memory integrity. В частности, missing interior token в непустом slot не обнаруживается cheap query-time shape check. Completeness `Ready` доверяется существующему authoritative lifecycle: full rebuild из forward postings и synchronized mutation maintenance.

### 5.5. Явная граница PR 5

PR 5 **не меняет**:

- `ContentIndex` persisted fields;
- `file_tokens_authoritative` lifecycle;
- `build_watch_index_from`;
- `schedule_rebuild_file_tokens`;
- startup `content_ready` timing;
- full/sync reindex publication;
- watcher locking strategy;
- lazy mutation rebuild safeguard;
- add/update/remove commit protocol.

Следовательно, PR 5:

- добавляет только query-local eligibility enum, не persisted/runtime index state machine;
- не требует generation counters;
- не требует rebuild controller, retry/backoff или spawn ownership;
- не вводит maintenance window;
- не меняет peak memory startup/reindex;
- не требует on-disk schema bump.

Production changes PR 5 ограничены planner/expansion/scoring telemetry. `src/lib.rs` и `src/mcp/watcher.rs` меняются только если нужен небольшой read-only helper или test instrumentation; mutation/rebuild production logic остается прежней.

### 5.6. Полнота reverse map и тестовый oracle

Existing authoritative contract должен быть закреплен тестами до включения scoped planner-а.

Test-only full verifier:

```text
для каждого forward (token, posting.file_id):
    reverse[file_id] содержит token
для каждого reverse (file_id, token):
    forward[token] содержит posting для file_id
```

Обязательные characterization/property tests:

1. Fresh `build_watch_index_from` до rebuild: RebuildPending/global fallback.
2. Completed rebuild: Ready/scoped candidate.
3. Background rebuild whole-vector visibility: query не видит partial slots.
4. Spawn-failure state до первого edit: RebuildPending/global fallback.
5. Lazy first-edit rebuild завершает map до mutation.
6. Add new unique/shared token сохраняет bidirectional equivalence.
7. Update remove/add token сохраняет equivalence.
8. Remove/batch purge очищает обе стороны.
9. Clone/load дает Unavailable/global fallback.
10. Truncated vector и empty live slot дают Inconsistent/global fallback.
11. Fault injection удаляет interior token из непустого slot: cheap eligibility остается Ready, full verifier падает. Это честно фиксирует границу production detection.
12. Global-vs-scoped differential query после каждой successful mutation остается эквивалентным.

PR 5 не ухудшает existing assertion/failure behavior mutation helpers. Отдельное превращение panic-based mutation invariants в fallible transactional protocol является самостоятельной correctness задачей и не смешивается с query planner.

### 5.7. Отдельный follow-up для rebuild lock latency

Shared code comments фиксируют примерно трехсекундный reverse rebuild под write lock. Это стоит исследовать отдельно, но scoped token-regex PR не должен одновременно перепроектировать index publication.

Отдельный follow-up начинается только после baseline:

- startup/post-load query lockWaitMs;
- watcher writer lockWaitMs во время rebuild;
- rebuild duration и peak RSS;
- spawn-failure/lazy-rebuild frequency;
- full reindex availability semantics.

Возможные решения - pre-publication owned preparation, detached immutable snapshot или memory-gated replacement generation - требуют собственного concurrency/memory design review. Они не являются условием корректности PR 5, потому что до готовности reverse map planner использует global fallback.

## 6. Контракты, которые нельзя сломать

### 6.1. Search semantics

Для эквивалентного content-index snapshot должны совпасть:

- matched file paths;
- line numbers;
- occurrences;
- TF-IDF scores;
- ranking;
- `termsMatched`;
- OR/AND behavior;
- invert complement;
- count-only totals;
- resultStatus completeness/truncation.

### 6.2. Invalid regex precedence

Даже при пустом scope invalid pattern должен возвращать error:

```text
invalid regex + empty scope -> error
valid regex + empty scope   -> empty scoped result
```

Short-circuit до compile недопустим.

### 6.3. Scope coverage

Пустой execution scope не всегда означает одинаковый public status:

- missing positive `file/dir/ext` scope может стать `scope_not_found`;
- exclude-only scope может дать обычный complete empty result;
- invert рассчитывает complement относительно empty universe;
- unindexed file-list coverage остается отдельным источником partial/unknown status.

Short-circuit должен пропустить result через существующие finalize, invert, scope telemetry и coverage layers, а не формировать новый ad hoc JSON.

### 6.4. Response contract PR 2 и versioned изменение v2

Сохраняются без semantic change:

- `termsSearched` содержит raw patterns;
- `regexExpansion` остается bounded;
- `countOnly` не содержит token preview;
- preview не больше 20 tokens;
- response truncation остается последней защитой;
- `maxResults` не меняет полные search-result totals.

PR 4-5 изменяют смысл существующих expansion counters в зависимости от execution strategy:

- v1 `tokensExamined`, `matchedTokenCount` и preview всегда описывают global vocabulary;
- v2 empty-scope strategy описывает empty execution universe;
- v2 scoped strategy описывает resolved-file token universe;
- v2 global strategy сохраняет v1 global meaning;
- `previewTruncated` считает hidden execution tokens текущего accounting scope.

Это не "только additive fields", а versioned semantic contract change. Клиент обязан проверять `regexExpansion.schemaVersion` и `accountingScope`, прежде чем интерпретировать expansion counts как global. Клиент, который игнорирует неизвестные поля и предполагает старую global semantics, может неверно истолковать v2 counters; changelog и MCP guide должны явно описать migration.

Search results, ranking, occurrences и raw query representation остаются backward-compatible. Global matched token count не вычисляется scoped planner-ом только ради старого telemetry contract, потому что это уничтожило бы optimization.

### 6.5. MCP/CLI dedup policy

Общие compile и scan abstractions должны принимать явную policy:

```rust
enum RegexExpansionDedup {
    DeduplicateSorted,
    PreservePatternDuplicates,
}
```

Запрещено выводить policy из имени caller или `cfg(test)`.

### 6.6. Regex AND characterization

Текущая semantics не является полной pattern-aware Boolean algebra:

- MCP дедуплицирует expanded tokens;
- CLI сохраняет overlapping duplicates;
- `term_count_for_all` основан на числе raw patterns;
- `terms_matched` основан на execution token positions.

PR 4-5 не должны молча исправлять или унифицировать regex-AND. Existing characterization tests являются oracle. Любая будущая pattern-aware AND semantics требует отдельного product contract.

### 6.7. Global IDF

Даже если token найден через scoped `file_tokens`, scoring обязан использовать:

```text
global posting list length
current global total_docs convention
```

Scoped doc frequency изменит ranking и является отдельным search-semantics change.

### 6.8. Snapshot consistency

Scope, reverse tokens, forward postings и file token counts читаются под одним content-index read guard. Query не должен отпускать lock между planner и scoring и затем смешивать разные watcher generations.

## 7. Цели и SLO

### 7.1. Функциональные цели

- Empty scope не сканирует token vocabulary/postings.
- Narrow scope не сканирует global vocabulary при ready reverse map.
- Global fallback всегда доступен.
- Search results эквивалентны текущему global oracle.
- Telemetry объясняет strategy, universe и fallback.
- PR 4-5 не меняют on-disk schema.

### 7.2. Performance цели

На Shared-like corpus:

- K=0 broad regex: `tokensExamined=0`, `postingsChecked=0`.
- K=1 broad regex: `tokensExamined` близок к unique tokens выбранного файла, а не 3.9M.
- K=1 `postingListsVisited` близок к scoped matched token count.
- K=1/K=10 speedup минимум 5x; целевой 10x.
- K=1 p95 не хуже 1 second для broad `.*` на текущем Shared host.
- Unscoped regression не больше 10% median.
- Wide-scope planner не строит query-local union, если его estimated cost выше global scan.

### 7.3. Memory цели

- PR 4 не добавляет persistent memory.
- PR 5 использует уже существующий reverse map.
- Query-local transient memory bounded числом scoped token references/unique tokens.
- K=1 union не клонирует token strings.
- Process working-set delta для repeated K=1 не растет монотонно.
- Persistent IDs не принимаются без отдельного memory budget.

## 8. Cost model

Обозначения:

- $P$ - число regex patterns;
- $T$ - global unique tokens;
- $K$ - scoped files;
- $R_K$ - сумма token references в `file_tokens` выбранных K files;
- $U_K$ - unique token union выбранных files;
- $M_K$ - matching tokens внутри scoped union;
- $Q(t)$ - размер global posting list token $t$;
- $H_K$ - postings выбранных matching tokens, попавшие в scope.

### 8.1. Текущий путь

$$
T_{global} = O(P \cdot T)
           + O(E \log E)
           + O\left(\sum_{t \in E} Q(t)\right)
$$

где $E$ - global expanded token set.

### 8.2. Scoped string-token path

$$
T_{scoped} = O(R_K)
           + O(U_K \log U_K)
           + O(P \cdot U_K)
           + O(M_K \log M_K)
           + O\left(\sum_{t \in M_K} Q(t)\right)
$$

Для K=1 token slot уже unique/sorted, поэтому union может использовать borrowed slice без HashSet и без первой сортировки.

### 8.3. Остаточный риск

Scoped vocabulary устраняет обход posting lists tokens, отсутствующих в scope. Но для каждого scoped matching token все еще читается global posting list. Hot tokens могут оставить значительный residual:

$$
postingWaste_{scoped} = 1 -
\frac{H_K}{\sum_{t \in M_K} Q(t)}
$$

Только после измерения этого residual принимается решение о sorted postings/binary lookup или file-local posting references.

## 9. Архитектура planner-а

### 9.1. Strategy enum

```rust
enum TokenRegexExpansionStrategy {
    EmptyScope,
    ScopedFileTokens,
    GlobalVocabulary,
}
```

### 9.2. Planner reason

```rust
enum TokenRegexPlannerReason {
    EmptyResolvedScope,
    ScopeUnfiltered,
    ReverseMapUnavailable,
    ReverseMapInvalid,
    ScopeTooWide,
    TokenReferenceEstimateTooHigh,
    ScopedCostPreferred,
}
```

Имена public JSON должны быть lower camel case и стабильными в пределах schema version.

### 9.3. Planner inputs

```rust
struct TokenRegexPlannerInput<'a> {
    scope: &'a ResolvedFileScope,
    global_unique_tokens: usize,
    files: &'a [String],
    file_token_counts: &'a [u32],
    file_tokens: &'a [Vec<String>],
    file_tokens_authoritative: bool,
}
```

### 9.4. Planner output

```rust
struct TokenRegexExpansionPlan<'a> {
    strategy: TokenRegexExpansionStrategy,
    reason: TokenRegexPlannerReason,
    scope_files: usize,
    scope_token_references: usize,
    token_universe: TokenUniverse<'a>,
}

enum TokenUniverse<'a> {
    Empty,
    SingleFile(&'a [String]),
    ScopedUnion(Vec<&'a str>),
    Global,
}
```

### 9.5. Cost decision

Initial threshold не является публичным контрактом. Он калибруется Criterion и Shared runs.

Консервативный v1 planner:

```text
если scope empty:
    EmptyScope
иначе если scope All:
    GlobalVocabulary
иначе если reverse map not ready/consistent:
    GlobalVocabulary
иначе вычислить R_K saturating sum
если R_K >= global_unique_tokens * scoped_reference_ratio:
    GlobalVocabulary
иначе:
    ScopedFileTokens
```

Начальный benchmark candidate для `scoped_reference_ratio` - 25%. Merge допускается только после сравнения 10%, 25%, 50% и adaptive estimate.

### 9.6. Single-file fast path

Для K=1:

- token slot уже sorted/dedup;
- regex сканирует borrowed `&String`/`&str`;
- matching strings клонируются только для final execution expansion;
- дополнительный union HashSet не нужен;
- `scopeUniqueTokens == scopeTokenReferences`.

### 9.7. Multi-file union

Для K>1 возможны две реализации:

1. `HashSet<&str>` с последующим sorted collect.
2. K-way merge sorted/dedup slots.

K-way merge предпочтительнее по allocation/locality, но сложнее. PR 5 может начать с `HashSet<&str>`, если benchmark показывает приемлемую стоимость для K<=10/1%.

Нельзя использовать `HashSet<String>`: это клонирует каждый token до regex match и повторяет текущую amplification в меньшем масштабе.

## 10. Refactor shared token-regex scanner

### 10.1. Разделение compile и scan

Предлагаемые abstractions:

```rust
struct CompiledTokenRegex {
    patterns: Vec<regex::Regex>,
}

fn compile_token_regex_patterns(
    raw_terms: &[String],
) -> Result<CompiledTokenRegex, RegexExpansionError>;

fn expand_compiled_token_regex<'a, I>(
    compiled: &CompiledTokenRegex,
    tokens: I,
    dedup: RegexExpansionDedup,
) -> RegexExpansion
where
    I: IntoIterator<Item = &'a str>;
```

### 10.2. Почему compile должен быть отдельным

- invalid regex precedence сохраняется при empty scope;
- compile timing измеряется точно;
- один compiled set используется global/scoped strategy;
- CLI и MCP не расходятся по regex flags/anchors;
- benchmark может изолировать scan от compile.

### 10.3. Dedup implementation

Для MCP:

```text
collect matching strings
sort
stable deterministic dedup
```

Для CLI:

```text
для каждого pattern пройти global token universe
сохранить pattern duplicates
```

Scoped strategy на первом этапе применяется только MCP. CLI продолжает global duplicate-preserving path, пока для CLI не будет отдельно спроектирован file scope planner.

### 10.4. Pattern match counts

`pattern_match_counts` в scoped strategy описывает matches внутри execution universe, не global vocabulary. Это необходимо отметить в telemetry schema. Использовать эти counts для глобальных exhaustive conclusions нельзя.

## 11. PR 4: empty scope и phase telemetry

### 11.1. Цель

Сделать K=0 дешевым и получить точное phase evidence для PR 5/6, не меняя planner для non-empty scope.

### 11.2. Control flow

```text
raw_terms validation
  -> compile regex patterns
  -> if file_scope.is_empty():
         create empty RegexExpansion(strategy=emptyScope)
         skip vocabulary scan
         skip posting score
     else:
         current global vocabulary expansion
         current scoring
  -> existing finalize/build/invert/scope/coverage pipeline
```

### 11.3. Invalid regex

Обязательный порядок:

```text
compile -> empty-scope check -> expansion
```

Тест:

```text
invalid pattern + missing file scope => ToolCallResult::error
```

### 11.4. Phase telemetry

`regexExpansion` получает versioned v2 block. Поля bounded, но смысл counters явно зависит от `accountingScope`:

```json
{
  "schemaVersion": 2,
  "strategy": "emptyScope|globalVocabulary|scopedFileTokens",
  "strategyReason": "emptyResolvedScope|...",
  "accountingScope": "none|globalVocabulary|resolvedFiles",
  "patterns": 1,
  "tokensExamined": 0,
  "matchedTokenCount": 0,
  "postingListsVisited": 0,
  "postingsChecked": 0,
  "postingsInScope": 0,
  "timings": {
    "compileMs": 0.02,
    "planMs": 0.0,
    "universeBuildMs": 0.0,
    "scanCollectMs": 0.0,
    "sortDedupMs": 0.0,
    "expansionTotalMs": 0.0,
    "postingScoreMs": 0.0
  }
}
```

Все timings serial wall time. Они не суммируются с parallel worker CPU.

Точные границы:

- `compileMs`: только построение всех `regex::Regex`;
- `planMs`: от входа в planner до выбора strategy/reason;
- `universeBuildMs`: построение borrowed scoped union или zero для global/empty;
- `scanCollectMs`: regex matching и conditional clone matching strings;
- `sortDedupMs`: deterministic sort/dedup finalized MCP expansion;
- `expansionTotalMs`: wall time от входа в planner после compile до готового finalized expansion vector; включает plan, universe build, scan/collect и sort/dedup;
- `postingScoreMs`: весь `score_normal_token_search`, измеряется вне expansionTotal.

`searchTimeMs` дополнительно включает scope/coverage, finalize, sorting результатов и response preparation, поэтому сумма перечисленных phases не обязана равняться searchTimeMs.

### 11.5. Почему `scanCollectMs` объединен

Regex match и conditional clone выполняются внутри одного loop. Попытка измерять collection отдельным `Instant` на каждый match исказит hot path. Isolated benchmark может отдельно оценить clone cost.

### 11.6. Empty-scope response

PR 4 не создает новый response builder. Он формирует обычный empty result и затем применяет:

- `apply_invert`;
- scope telemetry;
- scope coverage;
- generic resultStatus;
- byte truncation.

Так сохраняется различие `scope_not_found`, exclude-only empty и invert empty universe.

### 11.7. PR 4 tests

Unit:

1. Compile valid/invalid pattern отдельно от scan.
2. Empty expansion имеет zero counters.
3. Global scanner сохраняет существующий output/dedup.
4. CLI duplicate-preserving wrapper не меняется.

Handler:

1. Missing file + valid `.*`:
   - `tokensExamined=0`;
   - `postingListsVisited=0`;
   - `postingsChecked=0`;
   - `scope_not_found` сохраняется.
2. Missing file + invalid regex:
   - error, не `scope_not_found`.
3. Exclude-only empty scope:
   - существующий status contract.
4. Empty scope + invert:
   - empty complement и корректный accounting.
5. Count-only:
   - preview отсутствует.
6. Non-empty K=1/global:
   - output до PR 4 эквивалентен после удаления новых timings.

### 11.8. PR 4 benchmark

Расширить `token_regex_expand`:

```text
token_regex_empty_scope/{T}/{patterns}
token_regex_phase/{T}/{selectivity}
```

Размеры:

- 1k;
- 10k;
- 50k;
- optional 1M synthetic для release-only local run.

Selectivity:

- 0%;
- 1%;
- 50%;
- 100%.

### 11.9. PR 4 acceptance

- B0 Shared: `tokensExamined=0`, `postingsChecked=0`.
- Invalid regex precedence сохранен.
- A1/B1/Ball semantic output не меняется.
- `regexExpansion.schemaVersion=2` и v1->v2 counter semantics документированы; search-result contract не меняется.
- No schema bump/reindex.
- Strict Clippy и full tests зеленые.

## 12. PR 5: scoped expansion через `file_tokens`

### 12.1. Цель

Сделать regex expansion зависимым от token universe resolved scope, если reverse map ready и scope достаточно узок.

### 12.2. Correctness argument

Пусть token $t$ отсутствует во всех scoped files. Тогда ни один posting $t$ не пройдет `scope.contains(file_id)`. Исключение такого token до scoring не может изменить scoped search result.

Следовательно:

```text
global regex matches
  intersect tokens present in scope
```

эквивалентно:

```text
regex scan union(tokens in scope)
```

для result paths, occurrences и scores, если scoring сохраняет global posting list и global IDF.

### 12.3. Planner integration

Planner вызывается после compile и до expansion:

```text
compiled = compile(raw_patterns)
plan = plan_token_regex_expansion(index, scope)
expansion = execute_plan(compiled, plan)
score(expansion.tokens, global_index, scope)
```

### 12.4. Fallback

Global path обязателен при:

- unscoped/All request;
- eligibility не равен `Ready`;
- reverse map Unavailable/RebuildPending/Inconsistent;
- обнаруженный shape mismatch;
- scope references выше threshold;
- configured test override `forceGlobal` только под `cfg(test)`.

Runtime environment switch не нужен для обычного API. Для rollout может быть временный server flag, но он не должен стать persisted product contract.

### 12.5. Telemetry semantics v2

При scoped strategy:

- `tokensExamined` = scoped unique token universe;
- `matchedTokenCount` = matching execution tokens in scope;
- preview = scoped matching token preview;
- `accountingScope = "resolvedFiles"`;
- global matched token count неизвестен без global scan;
- `globalMatchedTokenCountKnown = false`.

При global strategy:

- `accountingScope = "globalVocabulary"`;
- current counts сохраняются;
- `globalMatchedTokenCountKnown = true`.

### 12.6. Дополнительные planner counters

```json
{
  "scopeFiles": 1,
  "globalUniqueTokens": 3912392,
  "scopeTokenReferences": 34,
  "scopeUniqueTokens": 34,
  "fallbackReason": null
}
```

Не возвращать token ID/path arrays.

### 12.7. Scoring

PR 5 сохраняет текущий `score_normal_token_search` как oracle:

- передает только scoped matching token names;
- берет postings из global `index.index`;
- использует global `postings.len()` для IDF;
- фильтрует postings через scope;
- формирует те же file entries.

### 12.8. Residual posting optimization

После PR 5 Shared B1 должен показать:

```text
postingListsVisited ~= scoped matched token count
postingsChecked = sum global postings только scoped matching tokens
```

Если `postingsChecked` остается большим из-за hot tokens, это отдельный measured trigger для:

- sorted posting lists + binary lookup by file ID;
- file-local posting references;
- token/file pair index.

Не включать этот rewrite в PR 5 без нового benchmark.

### 12.9. PR 5 differential tests

На одном synthetic snapshot каждый query выполняется принудительно двумя planners:

```text
GlobalVocabulary oracle
ScopedFileTokens candidate
```

Сравниваются после удаления strategy-specific telemetry:

- paths;
- lines;
- occurrences;
- score bits или documented float tolerance;
- ranking;
- total files/occurrences;
- resultStatus;
- missing/partial coverage;
- invert output.

Матрица:

1. OR, один pattern.
2. OR, overlapping patterns.
3. AND characterization.
4. Selectivity 0/1/50/100%.
5. K=1, K=2, K=all.
6. Tokens shared between files.
7. Tokens unique to one file.
8. Empty source file.
9. Tombstoned file slot.
10. Count-only and ordinary response.
11. maxResults 0/1.
12. Invalid regex.

### 12.10. PR 5 reverse-map characterization tests

Production watcher lifecycle не меняется. Tests доказывают, когда query planner может доверять существующему map:

1. Fresh `build_watch_index_from` до rebuild: `RebuildPending`, global fallback.
2. Completed background/lazy rebuild: `Ready`, scoped planner разрешен.
3. Clone/load: `Unavailable`, global fallback.
4. Truncated vector: `Inconsistent(lengthMismatch)`, global fallback.
5. Empty live slot с positive count: `Inconsistent(emptyLiveSlot)`, global fallback.
6. Add file с new unique token: full verifier и global/scoped query equivalence.
7. Add file с existing hot token: equivalence.
8. Update removes old token and adds new token: equivalence.
9. Remove clears reverse slot and postings: equivalence.
10. Rename preserves correct identity/scope.
11. Batch purge нескольких files.
12. Repeated update не создает duplicates.
13. Spawn-failure + first edit lazy rebuild сохраняет current behavior и после rebuild дает Ready.
14. Query, получивший read lock до rebuild, использует global fallback.
15. Query после exclusive rebuild видит whole map; partial vector не наблюдается.
16. Fault injection удаляет interior token из непустого slot: cheap eligibility остается Ready, test-only full verifier падает. Это фиксирует границу detection, а не обещает query-time corruption scan.

### 12.11. PR 5 benchmarks

Новая Criterion group:

```text
token_regex_scope_strategy/{T}/{K}/{selectivity}/{strategy}
```

Параметры:

- $T$: 1k, 10k, 50k, optional 1M;
- $K$: 0, 1, 10, 1%, 10%, all;
- selectivity: 0%, 1%, 50%, 100%;
- strategy: forced global, forced scoped, planner-selected;
- token sharing: low, medium, hot-token-heavy.

Отдельно измерять:

- plan time;
- union time;
- regex scan/collect;
- sort/dedup;
- posting score;
- transient allocations.

### 12.12. Shared release gate

Повторить A/B/C/D corpus:

- один cold;
- 20 warm;
- median/p95;
- same target file;
- same index generation;
- последовательные runs.

PASS:

- A1/B1 strategy = `scopedFileTokens`;
- A1/B1 tokensExamined близок к `scopeUniqueTokens`;
- B1 postingListsVisited близок к scoped matched token count;
- B1 median speedup >=5x, target >=10x;
- B1 p95 <=1 second;
- Ball strategy = global и regression <=10%;
- result/ranking equivalent;
- process RSS после repeated runs не растет монотонно.

## 13. PR 6: persistent compact reverse-token IDs, условно

### 13.1. Условие старта

PR 6 начинается только если выполнены оба условия:

1. PR 5 показывает устойчивый K=1/K=10 speedup и correctness equivalence.
2. String-based `file_tokens` дает неприемлемый memory/startup/watcher overhead либо отсутствует в нужных read-only deployments.

### 13.2. Почему нужен format bump

Текущий persisted content index хранит:

```text
token String -> Vec<Posting>
```

Стабильного token ID namespace нет. Reverse IDs требуют как минимум:

```text
token_id -> token string
file_id -> token_id[]
token string -> token_id lookup
```

Все структуры одной generation должны загружаться атомарно и проходить range validation. Это изменение persisted layout и требует bump `CONTENT_INDEX_VERSION`.

### 13.3. Candidate representation

```rust
struct TokenTable {
    strings: Vec<String>,
    lookup: HashMap<String, u32>,
}

struct FileTokenIds {
    offsets: Vec<u64>,
    token_ids: Vec<u32>,
}
```

CSR-like representation предпочтительнее `Vec<Vec<u32>>` для persisted memory locality и меньшего allocator overhead.

### 13.4. ID stability

Token IDs не обязаны быть публично стабильны между rebuild, но обязаны быть internally consistent внутри index generation. Save/load tests должны проверять:

- every ID in range;
- offsets monotonic;
- last offset equals token_ids length;
- no duplicate token ID per file;
- each reverse pair соответствует forward posting;
- tombstoned files имеют empty range.

### 13.5. Watcher lifecycle

Новые tokens получают append-only IDs либо вызывают generation-local table rebuild. Удаление последнего posting token может оставить token tombstone до compaction. Нельзя переиспользовать ID в той же generation без полного remap reverse arrays.

### 13.6. Memory estimate

Нижняя граница только reverse IDs:

$$
M_{ids} \approx 4P + 8(F+1)
$$

Для Shared $P \approx 14.9M$, $F \approx 82k$:

$$
M_{ids} \approx 57 MiB
$$

Дополнительно требуются token strings, lookup, offsets, capacities и forward postings. Сравнивать нужно с фактическим memory string-based `file_tokens`, а не с нулем: current watch server уже хранит duplicated token strings в reverse map.

### 13.7. Decode и resource bounds

PR 6 обязан перенести существующий bounded-decode contract на новый payload:

- общий uncompressed decode budget не выше существующего `MAX_DECODE_BYTES` (сейчас 2 GiB), если отдельный lower product limit не принят;
- compressed и declared uncompressed lengths проверяются до allocation/decompression;
- сумма shard lengths считается checked arithmetic без overflow;
- offsets, counts и byte lengths проходят checked integer conversions;
- vector lengths ограничиваются до allocation;
- decompression output обязан точно совпасть с declared bounded length;
- token IDs и file offsets валидируются до публикации generation;
- ни один partially validated arena/reverse array не становится доступен queries;
- malformed/corrupt input вызывает полный load отказ и rebuild, не partial recovery;
- fuzz/property tests покрывают oversized lengths, overflow, truncated payload, invalid offsets и out-of-range IDs.

### 13.8. PR 6 gates

- Full build regression <=20%.
- Load regression <=20%.
- Single-file watcher update p95 regression <=20%.
- Peak/steady RSS budget согласован до merge.
- Persisted size growth измерен.
- Old format автоматически rebuild-ится.
- Rollback/rebuild path документирован.
- Corrupt ID/offset data не загружается partial.
- Bounded decode/decompression tests зеленые.
- PR 5 query results unchanged.

## 14. Telemetry contract

### 14.1. Schema version и migration

Добавить `regexExpansion.schemaVersion=2` в PR 4, когда empty-scope counters впервые перестают описывать global scan. PR 5 расширяет допустимый `accountingScope` значением `resolvedFiles`, не меняя version повторно.

Интерпретация:

| schemaVersion | accountingScope | Semantics counters/preview |
|---:|---|---|
| отсутствует / 1 | implicit global | global token vocabulary |
| 2 | `none` | empty execution universe |
| 2 | `globalVocabulary` | global token vocabulary |
| 2 | `resolvedFiles` | unique tokens resolved file scope |

Migration requirements:

- changelog отдельно отмечает semantic change существующих fields;
- `docs/mcp-guide.md` показывает v1/v2 examples;
- tests проверяют все три v2 accounting scopes;
- compatibility snapshot подтверждает, что clients, использующие только search results и игнорирующие `regexExpansion`, не ломаются;
- нельзя обещать global `matchedTokenCount` при scoped strategy;
- `globalMatchedTokenCountKnown=false` обязателен для `resolvedFiles`;
- removal v1 compatibility aliases допускается только отдельным release notice, если aliases вообще понадобятся.

### 14.2. Постоянные поля

Поля constant-size:

```text
schemaVersion
strategy
strategyReason
accountingScope
patterns
tokensExamined
matchedTokenCount
postingListsVisited
postingsChecked
postingsInScope
globalUniqueTokens
scopeFiles
scopeTokenReferences
scopeUniqueTokens
globalMatchedTokenCountKnown
fallbackReason
timings
```

### 14.3. Preview

- ordinary response: максимум 20 matching execution tokens;
- count-only: preview отсутствует;
- `previewTruncated` считает hidden tokens текущего accounting scope;
- полный token array никогда не сериализуется.

### 14.4. Timing invariants

Compile находится до expansion envelope:

$$
expansionTotalMs \ge
planMs + universeBuildMs + scanCollectMs + sortDedupMs
$$

Допускается небольшой positive residual из-за orchestration и counter bookkeeping. `compileMs` и `postingScoreMs` находятся вне `expansionTotalMs`.

Для empty scope:

```text
universeBuildMs ~= 0
scanCollectMs = 0
sortDedupMs = 0
postingScoreMs = 0
```

Для global strategy `universeBuildMs=0`; traversal `index.index.keys()` относится к `scanCollectMs`.

Нельзя выдавать сумму per-token micro-timers как wall time.

### 14.5. Planner diagnostics

Fallback должен быть видимым и bounded:

```json
{
  "strategy": "globalVocabulary",
  "strategyReason": "reverseMapUnavailable",
  "fallbackReason": "fileTokensUnavailable"
}
```

Это позволяет отличить planner decision от regression или stale reverse map.

## 15. Test strategy

### 15.1. Unit tests

- compile valid/invalid patterns;
- dedup policy MCP/CLI;
- empty universe;
- global/scoped iterator expansion;
- deterministic preview;
- phase counters/timings non-negative;
- planner strategy/reason matrix;
- cost arithmetic saturating;
- eligibility Ready/Unavailable/RebuildPending/Inconsistent matrix;
- cheap shape diagnostics;
- test-only full bidirectional forward/reverse verifier;
- existing clone/load/authoritative lifecycle characterization.

### 15.2. Differential tests

Global planner остается correctness oracle. Candidate scoped output сравнивается с ним на одном immutable snapshot. Strategy-specific telemetry исключается из semantic comparison.

### 15.3. Property tests

Генерировать:

- files;
- per-file token sets;
- global postings;
- overlapping regex patterns;
- random scopes;
- add/update/remove sequences.

Проверять:

```text
scoped_result == global_result filtered by same scope
```

### 15.4. Concurrency characterization

PR 5 не меняет locking. Tests закрепляют существующее поведение:

- query держит read lock на planner + expansion + scoring;
- watcher writer ожидает active query;
- background rebuild держит exclusive write lock и не публикует partial reverse slots;
- query до rebuild использует global fallback;
- query после rebuild использует scoped strategy;
- query во время rebuild может ждать lock: это известный pre-existing latency, не новая гарантия PR 5;
- mutation и query не видят mixed forward/reverse state при существующих successful paths;
- PR 5 benchmark отдельно подтверждает отсутствие дополнительного lock hold относительно baseline.

### 15.5. Failure tests

- corrupted/truncated reverse vector;
- missing interior token в непустом slot: shape checks не находят defect, full bidirectional verifier находит;
- clone/load policy или reverse vector случайно не сброшены в Unavailable;
- authoritative empty vector неверно классифицирован как Ready;
- out-of-range scope ID;
- empty token slot при positive token count;
- regex compile error;
- response truncation;
- poisoned lock behavior сохраняет текущий contract.

## 16. Benchmark protocol

### 16.1. Criterion

Каждый benchmark фиксирует:

- Rust toolchain;
- build profile;
- corpus seed;
- T, F, P, K;
- selectivity;
- token-sharing distribution;
- strategy.

### 16.2. Shared runs

Для каждого case:

1. Один cold run.
2. 20 sequential warm runs.
3. Median и nearest-rank p95.
4. `xray_info` до и после.
5. Никаких reindex/mutation между comparison runs.
6. Один и тот же target file/path.

### 16.3. Метрики

- totalTimeMs;
- searchTimeMs;
- phase timings;
- tokensExamined;
- scope token refs/unique;
- matching tokens;
- postings visited/checked/in scope;
- response bytes;
- working set/commit delta;
- planner strategy/reason.

## 17. Rollout и rollback

### 17.1. PR 4

Rollback возвращает global expansion после valid compile. Нет persisted data и reindex.

### 17.2. PR 5

Global fallback остается в production. При regression planner можно временно принудить к global strategy server-side switch-ом без изменения index format. Switch должен быть удален после одного release cycle.

### 17.3. PR 6

Rollback old binary требует rebuild старого content-index format. Этот факт обязателен в changelog/release notes.

## 18. Security и resource bounds

- Regex crate сохраняет linear-time guarantees, но compile error должен быть bounded и early.
- Нельзя silent-cap expansion: это создаст false negatives.
- Query-local union allocation должен проверяться до построения через R_K estimate.
- Saturating counters обязательны для refs/postings/timings accounting.
- Hard resource limit, если понадобится, возвращает error или truthful partial с opt-in, но не silently incomplete result.
- Response остается bounded producer-side.

## 19. Затрагиваемые файлы

### PR 4

```text
src/mcp/handlers/grep.rs
src/mcp/handlers/token_regex.rs
src/mcp/handlers/grep_tests.rs
src/mcp/handlers/grep_tests_additional.rs
src/mcp/handlers/handlers_tests_grep.rs
benches/search_benchmarks.rs
CHANGELOG.md
```

### PR 5

```text
src/mcp/handlers/grep.rs
src/mcp/handlers/token_regex.rs
src/mcp/handlers/file_scope.rs          # только если нужен planner helper
src/mcp/handlers/grep_tests*.rs
src/mcp/handlers/handlers_tests_grep.rs
benches/search_benchmarks.rs
CHANGELOG.md
docs/mcp-guide.md                       # telemetry/accounting semantics
```

`src/lib.rs`, `src/mcp/watcher.rs` и `src/mcp/watcher_tests.rs` допускаются только для read-only eligibility helper или characterization tests. Изменение production rebuild/mutation lifecycle выводит change set за границу PR 5 и требует отдельного design review.

### PR 6, условно

```text
src/lib.rs
src/index.rs
src/index_tests.rs
src/mcp/watcher.rs
src/mcp/watcher_tests.rs
src/mcp/handlers/grep.rs
src/mcp/handlers/token_regex.rs
benches/search_benchmarks.rs
CHANGELOG.md
docs/storage.md
docs/mcp-guide.md
```

## 20. Пошаговый порядок реализации

### PR 4

1. Добавить red test: valid broad regex + empty scope все еще имеет nonzero `tokensExamined`.
2. Разделить regex compile и scan.
3. Повторить red test: invalid regex precedence.
4. Добавить empty-scope expansion strategy.
5. Пропустить empty result через existing finalization/coverage.
6. Добавить phase timing struct и JSON.
7. Мигрировать existing scanner tests на explicit dedup policy.
8. Добавить Criterion empty/phase groups.
9. Запустить focused tests, strict Clippy, full suite.
10. Выполнить Shared A0/B0 validation.
11. Independent Rust review.

### PR 5

1. Добавить internal forced-strategy test hook.
2. Создать planner enum/reasons/output.
3. Добавить reverse-map readiness helper.
4. Реализовать K=1 borrowed token universe.
5. Добавить global-vs-scoped differential tests.
6. Реализовать K>1 query-local union.
7. Добавить threshold planner и fallback telemetry.
8. Добавить reverse-map lifecycle characterization и query equivalence tests.
9. Добавить Criterion strategy matrix.
10. Запустить strict Clippy/full suite.
11. Выполнить Shared A/B/C/D validation.
12. Independent Rust review.

### PR 6

Не планировать implementation tasks до письменного decision record после PR 5 measurements.

## 21. Decision gates

### 21.1. После PR 4

Перейти к PR 5, если:

- empty scope исправлен;
- phase timings показывают multi-second non-empty broad cost;
- current global path semantic regression отсутствует.

### 21.2. После PR 5

Остановиться без PR 6, если:

- K=1/K=10 latency соответствует SLO;
- string reverse map memory приемлема;
- startup/rebuild/watch latency приемлема;
- residual postings не dominant.

Исследовать posting lookup, если:

- scoped tokens уже малы;
- `postingScoreMs` остается dominant;
- `postingsChecked / postingsInScope` остается высоким.

Исследовать persistent IDs, если:

- scoped strategy полезна;
- reverse map нужен в read-only deployments;
- string duplication/RSS или rebuild cost неприемлемы.

### 21.3. Запрет преждевременного format bump

Нельзя принимать PR 6 только на основании B1 posting waste до PR 5. B1 включает global tokens, которых не должно быть в scoped expansion. Сначала нужно измерить residual по scoped matching token set.

## 22. Definition of Done

### PR 4 готов, когда

- [ ] Valid regex + empty scope не сканирует vocabulary/postings.
- [ ] Invalid regex + empty scope остается error.
- [ ] Existing scope coverage/resultStatus contracts сохранены.
- [ ] Phase telemetry bounded; `regexExpansion` v2 semantic migration документирована и протестирована.
- [ ] Global non-empty output эквивалентен baseline.
- [ ] Criterion и Shared A0/B0 подтверждают improvement.
- [ ] Strict Clippy/full suite/review зеленые.

### PR 5 готов, когда

- [ ] Planner выбирает scoped strategy только при eligibility `Ready`.
- [ ] Global fallback покрывает все unsupported states.
- [ ] K=1/K>1 differential tests зеленые.
- [ ] OR/AND/invert/countOnly/ranking не изменились.
- [ ] Existing reverse-map lifecycle characterization, mutation equivalence и fault-injection tests зеленые.
- [ ] Shared B1 speedup >=5x и p95 <=1 second.
- [ ] Ball regression <=10%.
- [ ] Memory не растет монотонно.
- [ ] Telemetry честно сообщает accounting scope.
- [ ] Strict Clippy/full suite/review зеленые.

### PR 6 допускается, когда

- [ ] Есть письменный benchmark decision record.
- [ ] Memory/startup problem string map доказана.
- [ ] Storage format/migration/rollback спроектированы.
- [ ] Watcher lifecycle и corruption validation спроектированы.

## 23. Итоговое решение

Production evidence уже оправдывает два изменения без on-disk migration:

```text
PR 4:
compile regex
  -> empty scope: zero-work result
  -> non-empty: current global path

PR 5:
compile regex
  -> plan expansion universe
  -> narrow ready scope: file_tokens union
  -> otherwise: global vocabulary fallback
  -> global postings/IDF for matching execution tokens
```

Это минимальный путь, который:

- немедленно устраняет заведомо бессмысленную K=0 работу;
- превращает K=1 regex cost из функции global vocabulary в функцию scoped token universe;
- сохраняет current search semantics и global fallback;
- не требует reindex;
- дает измерения для решения реальной следующей bottleneck phase;
- откладывает persistent token IDs и sorted postings до доказанного residual.

Persistent format нужно менять только после того, как PR 5 докажет ценность scoped reverse vocabulary и покажет, что runtime string representation является следующим ограничением.
