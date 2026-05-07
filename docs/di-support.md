# DI Support — What xray Resolves Automatically (and What It Doesn't)

> **TL;DR.** xray does not parse your DI-container's registrations
> (`AddSingleton<IFoo, Foo>()`, `RegisterType<...>().As<...>()`, Lamar
> `For<...>().Use<...>()`, etc.). It is **receiver-type-driven**: at every
> call site it resolves the receiver's static type via AST, then matches it
> against `parent` class names and their declared `base_types`. Plus a few
> heuristics on top to bridge common DI-shaped cases (interface naming
> conventions, fuzzy field-name matching, in-method type inference for
> local variables).
>
> Net effect: **constructor / property / method / field injection through
> interfaces work out of the box across every mainstream .NET DI
> container** (Microsoft.Extensions.DependencyInjection, Autofac, Lamar,
> SimpleInjector, DryIoc, Ninject, Castle Windsor, Unity, MEF,
> StrongInject, Pure.DI, Jab, Scrutor). Service-locator-style code
> (`sp.GetRequiredService<T>()`, `scope.Resolve<T>()`, fluent factory
> chains) is **only partially** resolved — see the matrix below.

This document is the canonical reference for what `xray_callers` and
`xray_definitions` understand about dependency injection. Other documents
([architecture.md](architecture.md), [mcp-guide.md](mcp-guide.md),
[tradeoffs.md](tradeoffs.md)) link here instead of duplicating the matrix.

---

## How "DI awareness" actually works

xray has no DI registry. There is no parser path that consumes
`services.AddX<...>()` or `builder.RegisterType<...>().As<...>()` and
builds an `interface → impl` map. Everything labelled "DI-aware" in xray
is one of these four mechanisms, all driven by the existing AST and
content indexes:

1. **Direct `base_types` match at the call site** — implemented in
   [`resolve_call_site_via_base_types`](../src/mcp/handlers/callers.rs).
   For each candidate definition the tool checks two things:
   - the candidate's `parent` class name equals the receiver type (with
     a generic-arity guard so `new List<T>()` does not collide with a
     non-generic `List` class), or
   - the receiver type appears in the parent class's `base_types` (i.e.
     the parent class actually implements the interface used at the call
     site, e.g. `class FooService : IFoo`).
2. **Sibling-implementation expansion at depth 0** — implemented in
   [`expand_interface_callers`](../src/mcp/handlers/callers.rs). Only
   triggered when `resolveInterfaces=true` (default) and only at the
   root of the caller tree. Computes "related interfaces" for the target
   class via two heuristics, then re-runs the search through each:
   - the I-prefix convention: `Foo` ↔ `IFoo` (and the reverse for
     `IFoo` → `Foo`); and
   - the target class's own declared `base_types` (every interface it
     literally implements).
   Disabling it (`resolveInterfaces=false`) returns only the direct
   matches from mechanism 1.
3. **Fuzzy DI candidate-file expansion** — implemented near
   [`callers.rs:1622`](../src/mcp/handlers/callers.rs#L1622) via
   `find_implementations_of_interface` plus
   `collect_substring_file_ids`. When a `class` filter is supplied, the
   pre-filter that decides which files to scan is widened with:
   - files containing the I-prefixed name (`IClassName`),
   - files containing every implementation of that interface (looked up
     via `base_type_index`),
   - **trigram substring** matches on the class name and on
     `IClassName` — this is what catches DI-injected fields named
     `_userService`, `m_userService`, `userServiceField`, etc.
4. **Local-variable type inference inside one method** — implemented in
   [`extract_csharp_var_declaration_types`](../src/definitions/parser_csharp.rs#L1144)
   and [`collect_csharp_local_var_types`](../src/definitions/parser_csharp.rs#L1095).
   Lets the receiver of a chained call (`x.DoStuff()`) be resolved when
   `x` is a local that the analyser can type. See the next section for
   the exact list of patterns covered. (TypeScript has the analogous
   `extract_ts_var_declarator_type` / `resolve_ts_receiver_type`.)

These four mechanisms compose. A typical "DI works" flow for
`xray_callers method=["Charge"] class="StripePaymentApi"`:

- mechanism 3 widens the candidate file set to include every file that
  mentions `IPaymentApi`, every implementation of `IPaymentApi`, and
  every `_paymentApi` / `m_paymentApi` field;
- mechanism 1 verifies each call site by checking that the receiver type
  is `StripePaymentApi` *or* an interface listed in
  `StripePaymentApi.base_types` (so callers typed as `IPaymentApi` are
  picked up);
- mechanism 2 re-runs the same search through `IPaymentApi` itself, so
  a caller that holds an `IPaymentApi` field but never mentions
  `StripePaymentApi` is still found;
- mechanism 4 resolves `var pay = factory.Build(); pay.Charge();`-style
  call sites where it can.

What is missing from this picture: there is no step that reads
`services.AddSingleton<IPaymentApi, StripePaymentApi>()` and learns
"`IPaymentApi` ⇒ `StripePaymentApi`". xray relies on the actual
`: IPaymentApi` declaration on the class to make that link.

---

## C# — exact list of resolved patterns

### Local-variable type inference (mechanism 4)

`extract_csharp_var_declaration_types` walks every variable declaration
inside a method body (it does **not** recurse into nested
`local_function_statement`, `lambda_expression`, or
`anonymous_method_expression`). It populates a per-method
`name → type` map that the call-site resolver then consults when it
sees `<localName>.Method()`.

| Pattern | Resolved type | Notes |
|---|---|---|
| `IFoo x = expr;` | `IFoo` | Explicit annotation; first letter must be uppercase |
| `Foo<int> x = expr;` | `Foo` | Generic stripped to base name |
| `var x = new FooImpl();` | `FooImpl` | `object_creation_expression`; namespace-qualified `ns.FooImpl` collapsed to `FooImpl` |
| `var x = new FooImpl<T>(...);` | `FooImpl` | Generic stripped |
| `var x = (IFoo)expr;` | `IFoo` | `cast_expression` |
| `var x = expr as IFoo;` | `IFoo` | `as_expression` |
| `var x = SameClassMethod();` | declared return type of `SameClassMethod` | Only **same-class** methods — there is no global return-type table |
| `var x = await SameClassMethodAsync();` | `T` from `Task<T>` | `await_expression` + `unwrap_task_type` |
| `if (obj is FooImpl named) { named.X(); }` | `FooImpl` | `declaration_pattern` |
| `case FooImpl named: named.X();` | `FooImpl` | Same `declaration_pattern` path |
| `dynamic x = ...` | *not resolved* | Explicitly skipped |

**Cross-class method return types are not inferred.** If `GetFoo()`
lives on `IServiceFactory` and you write `var x = _factory.GetFoo();
x.DoStuff();`, the receiver of `x.DoStuff()` cannot be typed by xray
today. The same call written as `_factory.GetFoo().DoStuff();` (no
local) is not resolved either, for the same reason.

### Caller-tree direction = up

| Pattern | Works? | Why |
|---|---|---|
| Constructor injection (`public Service(IFoo foo)` / primary ctor) — `foo.X()` | ✅ | Field/parameter is typed `IFoo`; mechanism 1 matches via `base_types` |
| Property injection (`public IFoo Foo { get; set; }`) — `this.Foo.X()` / `Foo.X()` | ✅ | Same as above |
| Method injection (`void Run(IFoo foo)`) — `foo.X()` | ✅ | Parameter type is `IFoo` |
| Field with explicit interface type (`private readonly IFoo _foo;`) — `_foo.X()` | ✅ | Receiver resolved from field type |
| Decorator pattern (`class FooDecorator : IFoo`) | ✅ | The decorator implements `IFoo` so it shows up as one of the implementations |
| Strategy / chain of handlers | ✅ | `xray_definitions baseType=IFoo baseTypeTransitive=true` enumerates every implementation |
| Generic interfaces (`IRepository<Order>`, `IRepository<Customer>`) | ✅ | `baseType=IRepository` substring-matches both; with `baseTypeTransitive=true` it also walks the generic chain |
| Keyed services / `[FromKeyedServices("a")] IBar b` | ✅ for the call itself | Parameter is still `IBar` so the call site is picked up. The key string itself is invisible. |
| `IServiceProvider.GetRequiredService<IFoo>()` stored in a field | ✅ if assigned to a field | `_foo = sp.GetRequiredService<IFoo>(); ... _foo.X()` works because `_foo` is typed |
| Castle DynamicProxy / Autofac interceptors | ✅ when the proxy implements the same interface | The proxy still appears as an implementation of `IFoo` in `base_types` |
| Source-generated DI containers (StrongInject, Pure.DI, Jab) | ✅ if generated `.cs` files are on disk and not excluded | Generated files participate in the index like any other source. If `.gitignore` excludes `obj/Generated/`, they will be invisible — pass `--no-respect-gitignore` or whitelist the folder if you need them indexed |

### What does **not** work (or works only by accident through textual file pre-filter)

| Pattern | Why it fails | Workaround |
|---|---|---|
| `var foo = sp.GetRequiredService<IFoo>(); foo.X();` | `GetRequiredService<T>` is not in the known-factory list — the local can't be typed | Assign to a field once and read the field, or write `((IFoo)sp.GetRequiredService(typeof(IFoo))).X()` (the cast resolves) |
| `sp.GetRequiredService<IFoo>().X();` | Same as above; even if the local case worked, return-type-of-call-as-receiver is not currently tracked | Same workaround |
| `scope.Resolve<IFoo>().X()` (Autofac), `container.GetInstance<IFoo>().X()` (Lamar / SimpleInjector / StructureMap), `kernel.Get<IFoo>().X()` (Ninject) | All variants of the same service-locator pattern | Same workaround |
| `factory.Create().X()` where `Create()` lives on another class and returns `IFoo` | Cross-class return-type inference is not implemented | Store the result in a local: `var foo = factory.Create(); foo.X();` only helps when `factory` and `Create` are in the same class |
| Convention-based registrations (Autofac `RegisterAssemblyTypes(...).AsImplementedInterfaces()`, Lamar `Scan(...)`, Scrutor `services.Scan(...)`, MEF `[Export]`, MAUI/Blazor `[Inject]` registrations) | xray does not parse the *registration code* at all | Call sites still resolve normally because they go through the interface; if you specifically want "where is `IFoo` registered?" use `xray_grep terms=["AddSingleton", "RegisterType", "For<", "Scan"]` |
| `dynamic` dispatch | No static type to resolve | None — out of AST scope |
| Reflection (`Activator.CreateInstance`, `MethodInfo.Invoke`, `MakeGenericMethod`, expression trees) | Symbolic only — cannot be resolved without a runtime | None |
| `services.AddScoped(typeof(IRepo<>), typeof(EfRepo<>))` open-generic registration | The registration is invisible to xray. Call sites typed `IRepo<Order>` are still picked up via `base_types`; xray simply doesn't know which closed-generic implementation will be activated | Use `xray_definitions baseType=IRepo baseTypeTransitive=true` to enumerate implementations manually |

---

## TypeScript / Angular

The same model applies, with the relevant code in
`src/definitions/parser_typescript.rs`
(`extract_ts_var_declarator_type`, `resolve_ts_receiver_type`).

- Constructor-injected services (`constructor(private foo: IFoo) {}`) —
  `foo.x()` works through mechanism 1.
- Angular DI through `@Injectable()` + `providers: [...]` follows the
  same path: the constructor parameter is typed, the call site is
  resolved.
- TypeScript's structural typing means `base_types` is more permissive
  in spirit than the parser actually models — only `extends` /
  `implements` clauses are recorded, structural conformance through
  duck typing is **not** discovered.
- The Angular template tree (`<app-foo>` ↔ `FooComponent`) has its own
  resolver via `selector_index`, separate from the DI logic above. See
  [architecture.md — Angular Template Metadata](architecture.md#angular-template-metadata).

## Rust

Rust traits are recorded in `base_types` like C# interfaces, so
`impl Foo for Bar` will let `xray_callers` find calls on a `&dyn Foo`
receiver back to `Bar::method`. There is no separate "DI-aware" layer:
trait-object dispatch is the only mechanism, and `base_types` already
covers it. See `extract_rust_call_sites` in
`src/definitions/parser_rust.rs` for the call-site extractor and
`resolve_rust_receiver` for the receiver resolver.

## SQL

No DI concept. `xray_callers` works on stored-procedure call graphs via
the regex parser (`EXEC` / `EXECUTE` and call sites mined from `JOIN` /
`INSERT` / `UPDATE` bodies). This document does not apply.

---

## Recipes for DI-shaped code bases

### "Find every caller of a service method"

Prefer the **concrete implementation**, not the interface, as the entry
point. Mechanism 2 then expands to the interface for you:

```jsonc
{
  "method": ["Charge"],
  "class": "StripePaymentApi",   // concrete class
  "direction": "up",
  "resolveInterfaces": true,     // default — keep it on
  "depth": 5
}
```

If the response feels short, follow the advisory hint and re-run with
`class: "IPaymentApi"` to capture call sites that only ever name the
interface (mechanism 1's direct branch).

### "Enumerate every implementation of an interface"

```jsonc
{
  "baseType": "IPaymentApi",
  "baseTypeTransitive": true,    // also walks IPaymentApi : IPayment chains
  "kind": ["class"]
}
```

This call (on `xray_definitions`) reads the `base_type_index` directly
and is independent of `xray_callers` — useful as a pre-step to drive a
`xray_callers` per implementation.

### "Where is `IFoo` registered?"

xray does not model registrations. Use `xray_grep`:

```jsonc
// Microsoft.Extensions.DependencyInjection
{ "terms": ["AddSingleton<IFoo", "AddScoped<IFoo", "AddTransient<IFoo"] }

// Autofac
{ "terms": [".As<IFoo>", "RegisterType<", "RegisterAssemblyTypes"] }

// Lamar
{ "terms": ["For<IFoo", "Use<"] }

// Scrutor / convention scanning
{ "terms": ["services.Scan", "AsImplementedInterfaces"] }
```

For the AI-migration-planner workflow that combines this with caller
analysis, see
[use-cases.md — Common LLM Workflows](use-cases.md#common-llm-workflows-you-can-build-today).

### "I rely on `GetRequiredService<T>()` and the local doesn't resolve"

Two pragmatic options until that path is implemented:

1. Refactor the call site once to assign through a typed local with a
   cast: `var foo = (IFoo)sp.GetRequiredService(typeof(IFoo)); foo.X();`
   — the `cast_expression` path resolves.
2. For one-off investigations, fall back to `xray_grep`:
   `terms=["GetRequiredService<IFoo>", "Resolve<IFoo>"] lineRegex=true`.
   You lose call-tree context but get every call site.

Tracked as a known gap; see the user-stories folder for any in-flight
roadmap items.

---

## Related design docs

- [architecture.md — Caller Tree Verification](architecture.md#caller-tree-verification)
  — the canonical bullet list of caller-resolution mechanisms (kept in
  sync with this document).
- [tradeoffs.md — §10 Interface Resolution Depth](tradeoffs.md#10-interface-resolution-depth-in-caller-trees)
  — why `resolveInterfaces` only expands at depth 0 and what the
  combinatorial trade-off looks like.
- [mcp-guide.md — `xray_callers`](mcp-guide.md#xray_callers--call-tree-analysis)
  — protocol-level reference for `class`, `resolveInterfaces`, and the
  Limitations section.
