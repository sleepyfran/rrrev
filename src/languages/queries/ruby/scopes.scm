[
  (method)
  (singleton_method)
  (class)
  (module)
  (singleton_class)
] @scope

; Specs: `describe "…" do`, `it "…" do`.
(call
  method: (identifier) @_name
  block: (_) @scope.body
  (#any-of? @_name "describe" "context" "it" "test")) @scope.always
