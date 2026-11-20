[
  (namespace)
  (module_defn)
  (type_definition)
  (member_defn)
] @scope

; A function, or a value worked out over several lines; not `let x = 3`.
(function_or_value_defn (function_declaration_left)) @scope.always
(function_or_value_defn) @scope.multiline
