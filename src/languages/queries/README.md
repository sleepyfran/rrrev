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
