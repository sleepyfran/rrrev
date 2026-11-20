; tree-sitter-kotlin-ng ships no queries, so these are rrrev's own. Later
; patterns win over earlier ones for the same node.

(identifier) @variable

((identifier) @constant.builtin
  (#any-of? @constant.builtin "true" "false" "null"))

((identifier) @constant
  (#match? @constant "^[A-Z][A-Z0-9_]+$"))

; Types

(user_type (identifier) @type)
(class_declaration name: (identifier) @type)
(object_declaration name: (identifier) @type)
(type_alias type: (identifier) @type)
(enum_entry (identifier) @constant)

; Functions

(function_declaration name: (identifier) @function)
(call_expression (identifier) @function.call)
(call_expression
  (navigation_expression (identifier) @function.method.call .))

(parameter (identifier) @variable.parameter)
(class_parameter (identifier) @variable.parameter)
(lambda_parameters (variable_declaration (identifier) @variable.parameter))

(this_expression) @variable.builtin
(super_expression) @variable.builtin

; Literals

[
  (string_literal)
  (multiline_string_literal)
  (character_literal)
] @string

(string_content) @string
(escape_sequence) @string.escape
(interpolation ["$" "${" "}"] @punctuation.special)

[
  (number_literal)
  (float_literal)
] @number

[
  (line_comment)
  (block_comment)
] @comment

((block_comment) @comment.documentation
  (#match? @comment.documentation "^/\\*\\*"))

(shebang) @keyword.directive

; Annotations and labels

(annotation) @attribute
(label) @label

(package_header (qualified_identifier) @module)
(import (qualified_identifier) @module)

; Keywords

[
  "package" "import" "class" "interface" "object" "fun" "val" "var"
  "typealias" "constructor" "init" "companion" "by" "where" "get" "set"
  "enum" "annotation" "data" "value" "sealed" "inner" "open" "abstract"
  "final" "override" "private" "protected" "public" "internal" "lateinit"
  "const" "inline" "noinline" "crossinline" "tailrec" "suspend"
  "operator" "infix" "external" "expect" "actual" "vararg" "out" "in"
  "if" "else" "when" "for" "while" "do" "try" "catch" "finally" "throw"
  "return" "return@" "this@" "super@" "is" "as" "as?" "!is" "!in"
] @keyword

; Operators

[
  "+" "-" "*" "/" "%" "=" "+=" "-=" "*=" "/=" "%=" "++" "--" "&&" "||" "!"
  "!!" "==" "!=" "===" "!==" "<" ">" "<=" ">=" "?:" "?." "::" ".." "..<" "->"
] @operator
