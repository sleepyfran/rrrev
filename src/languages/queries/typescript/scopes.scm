[
  (function_declaration)
  (generator_function_declaration)
  (class_declaration)
  (abstract_class_declaration)
  (method_definition)
  (interface_declaration)
  (enum_declaration)
  (internal_module)
  (module)
] @scope

; A function given a name: `const f = () => {`, `k: function () {`, not every
; callback passed to `.map`.
(variable_declarator [(arrow_function) (function_expression)] @scope.always)
(pair [(arrow_function) (function_expression)] @scope.always)
(assignment_expression [(arrow_function) (function_expression)] @scope.always)
(public_field_definition [(arrow_function) (function_expression)] @scope.always)

; Test suites: `describe("…", () => {`, `it("…", function () {`.
(call_expression
  function: (identifier) @_name
  arguments: (arguments
    [
      (arrow_function body: (_) @scope.body)
      (function_expression body: (_) @scope.body)
    ] .)
  (#any-of? @_name "describe" "context" "it" "test")) @scope.always
