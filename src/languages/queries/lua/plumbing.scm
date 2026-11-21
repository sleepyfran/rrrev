; `local a = require("a")`
(variable_declaration
  (assignment_statement
    (expression_list
      .
      (function_call name: (_) @_name (#eq? @_name "require"))
      .))) @plumbing
