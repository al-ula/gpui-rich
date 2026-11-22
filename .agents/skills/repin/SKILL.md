---
name: repin
description: Fast repository intelligence engine for symbol search, AST inspection across 8+ languages, dependency and call path tracing, blast-radius impact analysis, and token-budgeted context packing for AI coding agents. Use when navigating large codebases, analyzing refactoring impact, finding shortest call paths, resolving symbol coordinates, or packing minimal relevant context into LLM prompts.
license: Apache-2.0 OR MIT
compatibility: Requires Linux x86_64 and Repin CLI (repin).
---

# Repin Agent Skill

Repin is a repository intelligence engine designed for autonomous coding agents and developers. It indexes symbol graphs, AST definitions, call hierarchies, and full-text search into an embedded SQLite database backed by a background daemon.

Use `repin` instead of brute-force directory traversal, unbounded `grep`, or reading entire files into prompt context.

---

## 1. Supported Languages & Extractors

Repin parses source trees into an authoritative symbol and dependency graph across 9 language packs:

| Language | Extensions | Graph Capabilities |
|---|---|---|
| **Rust** | `.rs` | Functions, structs, enums, traits, impls, modules, macro defs, call & import edges |
| **TypeScript / JS** | `.ts`, `.tsx`, `.js`, `.jsx`, `.mjs`, `.cjs` | Functions, classes, interfaces, types, methods, imports, exports, calls |
| **Python** | `.py` | Functions, classes, methods, imports, calls |
| **Go** | `.go` | Functions, methods, structs, interfaces, intra-package calls, type references |
| **C** | `.c`, `.h` | Functions, structs, unions, enums, typedefs, calls |
| **C++** | `.cpp`, `.cc`, `.cxx`, `.hpp`, `.hh`, `.hxx` | Functions, classes, structs, methods, namespaces, templates, calls |
| **Java** | `.java` | Classes, interfaces, enums, methods, constructors, imports, calls |
| **C#** | `.cs` | Classes, interfaces, structs, enums, methods, namespaces, calls |
| **Prose** | `.md`, `.txt`, `.rst` | Markdown headings, section hierarchy, lexical indexing |

---

## 2. Workspace Setup, Lifecycle & Freshness Rules

### Workspace Initialization & Status

```bash
# Verify installation and compatibility diagnostics
repin --version
repin version --json

# Initialize repository metadata and index workspace (creates .repin/)
repin init

# Check daemon health, graph revision (e.g. r715), node count, and edge count
repin status
```

### Daemon Lifecycle

```bash
# Check daemon process and socket state
repin daemon status

# Stop background daemon
repin stop

# Restart daemon using project database
repin restart
```

### Freshness & Synchronization Rules

1. **Automatic File Watching**: The background daemon monitors repository worktrees via filesystem events and advances graph revisions (`r<N>`) automatically.
2. **Do NOT run `repin sync` after every single file edit**: Unnecessary sync calls add latency without benefit.
3. **DO run `repin sync` when**:
   - You need an immediate synchronous freshness barrier right after writing code before querying.
   - After VCS branch changes, rebase, merge, or large checkout (`git checkout`, `git pull`).
   - If query results do not reflect an edit you just made.
4. **Rebuilding**: Run `repin rebuild <graph|lexical|vector|all>` when an index must be rebuilt from scratch.

---

## 3. Discovery & Search

### A. Search Channels & Modes

```bash
# Hybrid search (default): FTS5 lexical matching fused with graph degree-centrality ranking
repin search "<QUERY>"

# Symbol graph search: matches declared symbols (functions, types, traits, classes)
repin search -g "<SYMBOL_NAME>"

# Exact regular-expression search across the working tree
repin search -r "<REGEX>"

# Explicit search mode override: direct, regex, graph, or hybrid
repin search "<QUERY>" --mode hybrid

# Adjust graph degree-centrality boost (higher values prioritize connected architectural hubs)
repin search "<QUERY>" --boost 0.5

# Bound maximum matches (defaults to config limit: 50)
repin search "<QUERY>" -l 10
```

### B. Global Project & Config Flags

Run commands outside repository root by providing `-p, --project <PATH>` or `-c, --config <PATH>`:

```bash
repin search -p /path/to/repo -g "Engine"
```

---

## 4. AST Inspection & Coordinate Resolution

Inspect declared symbols, boundaries, and line ranges without reading entire files:

```bash
# Print file's structural AST outline (declarations, types, methods, line ranges)
repin inspect path/to/file.rs

# Resolve exact AST symbol definition at 1-based source coordinate (line & column)
repin at-position path/to/file.rs 42 10

# Lookup detailed metadata for an entity by symbol name or Node ID (node_...)
repin entity "SqliteStore"
repin entity "node_82003bf0168b"
```

`repin entity` returns:
- Qualified name and kind (`struct`, `fn`, `method`, `class`, `module`, etc.)
- File path and language
- Byte span (`span: start..end`)
- Exact 1-based line:column range (`Range: 13:1 - 15:2`)

---

## 5. Graph Relationships & Path Tracing (ADR-025)

### A. Direct Neighbors

```bash
# Inspect 1-hop callers/referrers and callees/dependencies
repin neighbors "<SYMBOL_NAME_OR_ID>"

# Traverse up to N hops of graph relationships
repin neighbors "<SYMBOL_NAME_OR_ID>" --max-depth 2
```

Outputs directional relations:
- Incoming: `<- [calls]`, `<- [imports]`, `<- [contains]`
- Outgoing: `-> [calls]`, `-> [imports]`, `-> [contains]`

### B. Shortest Path Tracing

Trace how two components connect through dependencies or calls:

```bash
# Find shortest call or dependency path connecting two symbols or files
repin path "<SOURCE_SYMBOL>" "<TARGET_SYMBOL>"

# Machine-readable JSON output
repin path "<SOURCE_SYMBOL>" "<TARGET_SYMBOL>" --json
```

**JSON Output Format:**

```json
{
  "status": "ok",
  "data": {
    "from": {
      "name": "crates/repin-cli/src/app.rs",
      "kind": "file",
      "path": "crates/repin-cli/src/app.rs"
    },
    "to": {
      "name": "SqliteStore",
      "kind": "struct",
      "path": "crates/repin-core/src/store/sqlite.rs"
    },
    "max_depth": 5,
    "paths": [
      {
        "length": 2,
        "segments": [
          {
            "node": { "name": "crates/repin-cli/src/app.rs", "kind": "file", "path": "crates/repin-cli/src/app.rs" },
            "edge_to_next": { "kind": "imports" }
          },
          {
            "node": { "name": "SqliteStore", "kind": "struct", "path": "crates/repin-core/src/store/sqlite.rs" },
            "edge_to_next": null
          }
        ]
      }
    ]
  }
}
```

---

## 6. Impact Analysis & Blast Radius (ADR-025)

Before refactoring, modifying, or deleting a symbol or file, analyze what will break:

```bash
# Analyze upstream callers/dependents and downstream dependencies (default depth: 3)
repin impact "<SYMBOL_OR_FILE>"

# Adjust traversal depth
repin impact "<SYMBOL_OR_FILE>" --max-depth 5

# Machine-readable JSON output for programmatic evaluation
repin impact "<SYMBOL_OR_FILE>" --json
```

**JSON Output Structure:**

```json
{
  "status": "ok",
  "data": {
    "items": [
      {
        "depth": 1,
        "node": {
          "id": [246, 36, 69, ...],
          "name": "tests",
          "qualified_name": "tests",
          "kind": "module",
          "language": "rust",
          "path": "crates/repin-core/src/context.rs",
          "range": {
            "start": { "line": 423, "column": 1 },
            "end": { "line": 586, "column": 2 }
          }
        },
        "via_edge_kind": "imports"
      }
    ]
  },
  "warnings": [],
  "provenance": { "sources": ["graph"] }
}
```

---

## 7. Budgeted Context Packing for LLMs & Tasks (ADR-016, ADR-017)

Assemble dense, relevant source ranges packed to fit within a strict byte/token budget:

```bash
# Assemble context for a task or query within a strict byte budget
repin context "daemon connection protocol negotiation" --budget 32768

# Tailor context generation:
# --padding-lines <N>    Add surrounding lines around matched definitions (default: 2)
# --no-blast-radius      Exclude caller/callee expansion; focus budget on direct hits
# --no-verbatim-source   Include structural outlines only without code blocks
repin context "sqlite WAL checkpointing" --budget 16384 --padding-lines 3 --no-blast-radius

# Assemble review context for changed files and their graph impact since a revision
repin review-context --since 1 --budget 65536
```

---

## 8. Agent-Assisted Reranking (ADR-022)

For model-assisted or agent-driven reranking of search candidates:

```bash
# Rerank candidates with an agent shell callback command
repin rerank "session eviction timer" --agent-cmd 'my-agent-reranker --json'

# Tune rerank parameters
repin rerank "session eviction timer" --top-n 10 --deadline-ms 2000
```

When candidates are omitted, Repin retrieves top search candidates automatically and sends a JSON-RPC request to stdin:
- Subprocess protocol: Repin sends `{"jsonrpc":"2.0","method":"repin/rerank","params":{"query":...,"candidates":[{"id":...,"content":...}]}}`
- Agent response on stdout: `{"jsonrpc":"2.0","result":{"ranked":[{"id":...,"score":0.95},...]}}`

Manage local model assets in `~/.cache/repin/models/`:
```bash
repin model list
repin model download <MODEL_NAME>
repin model remove <MODEL_NAME>
```

---

## 9. Configuration & Diagnostics

```bash
# Inspect merged effective configuration in TOML format
repin config show

# Validate configuration syntax and invariants
repin config validate

# Inspect SQLite identity and schema without graph activation
repin db inspect --json

# Run Precision-at-N retrieval evaluation suite
repin eval
```

---

## 10. Recommended Agent Decision Tree

| Goal | Recommended Command | Rationale |
|---|---|---|
| **Locate symbol declaration** | `repin search -g "<NAME>"` | Exact graph symbol match; no noise from text occurrences. |
| **Broad semantic discovery** | `repin search "<QUERY>"` | Hybrid FTS5 + graph centrality rank fusion. |
| **Inspect file without whole read** | `repin inspect <PATH>` | Structural outline with symbols and line ranges; saves tokens. |
| **Identify coordinates before edit** | `repin entity "<NAME>"` | Returns exact byte span and 1-based start/end line & column. |
| **Find symbol at cursor / error** | `repin at-position <PATH> <L> <C>` | Maps coordinate directly to AST node. |
| **Check 1-hop callers / callees** | `repin neighbors "<NAME>"` | Immediate incoming and outgoing graph edges. |
| **Assess refactoring blast radius** | `repin impact "<NAME>" --json` | Traverses upstream callers and downstream dependencies. |
| **Trace path between components** | `repin path "<FROM>" "<TO>" --json` | Shortest path with edge types (`calls`, `imports`, etc.). |
| **Pack code context for prompt** | `repin context "<TASK>" --budget <N>` | Bounded, relevance-ranked source snippets with blast radius. |
| **Review diff / PR changes** | `repin review-context --since <REV>` | Focuses on changed files and their graph impact. |
| **Immediate freshness barrier** | `repin sync` | Use only after batch file edits before immediate query. |
