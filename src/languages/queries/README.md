# Queries

Tree-sitter queries per language, one folder per `name` in `languages.toml`.

## `scopes.scm`

What counts as a definition, for breadcrumbs and `ze`. Captures:

- `@scope`: a definition, when it has a body (a `body` or `block` field, or a
  child whose kind ends in `_body`) or takes more than one line. Keeps out
  declarations like `mod a;` or `struct A;`.
- `@scope.always`: a definition wherever it matches.
- `@scope.multiline`: a definition only when it takes more than one line.
- `@scope.body`: where the definition's label stops, when it isn't the body
  rrrev would find on its own.

Captures starting with `_` are for predicates only.

## `highlights.scm`

Syntax highlighting, with the capture names the grammars' own queries use
(`@keyword`, `@function.method`, …). A capture without a colour of its own
takes its parent's: `@function.method.call` falls back to `@function.method`,
then `@function`. When several patterns capture the same node, the last one
wins; an inner node is drawn over the one around it.

Each file starts as a copy of the grammar's own `queries/highlights.scm` (all
MIT), at the commit `languages.toml` pins, with small fixes where its order
assumed the first pattern wins. Kotlin's grammar ships none, so its file is
rrrev's own.

## `plumbing.scm`

Imports, exports and the like: lines that wire code together without doing
anything. A hunk whose every changed line sits inside a `@plumbing` capture
(blank lines aside) is a plumbing hunk, and starts folded. Keep these strict,
since a capture that's too wide hides real changes:

- `@plumbing`: an import, a re-export, a package line, an include.

Captures starting with `_` are for predicates only.

## Inheriting

A first line `; inherits: a, b` puts those languages' queries of the same kind
first. `ecma` and `jsx` are shared by JavaScript, TypeScript and TSX and are
no language of their own.
