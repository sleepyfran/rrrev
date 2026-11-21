(function_declaration) @scope

; `{ f = function() … end }`, `local d = function() … end`, `M.e = function() … end`.
(field (function_definition) @scope.always)
(assignment_statement (_ (function_definition) @scope.always))
